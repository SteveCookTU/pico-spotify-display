use crate::access_token_response::AccessTokenResponse;
use crate::current_playing_track::CurrentPlayingTrackResponse;
use crate::BASIC_AUTH;
use core::fmt::Write as fmt_write;
use defmt::{info, warn};
use embassy_net::driver::Driver;
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_time::Duration;
use embedded_io_async::Write;
use embedded_nal_async::{Dns, TcpConnect};
use heapless::String;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::{Method, RequestBuilder};

static HTTP_RESP: &str = "HTTP/1.1 200 OK
Server: Pico
Content-Type: text/html
Content-Length: 10
Connection: Closed

Connected!";

pub async fn get_spotify_code<D: Driver>(stack: &Stack<D>, out_buf: &mut [u8]) -> usize {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("Listening on TCP:3000");
        if let Err(e) = socket.accept(3000).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Received connection from {:?}", socket.remote_endpoint());
        let mut buf = [0; 4096];
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) => {
                    warn!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("read error: {:?}", e);
                    break;
                }
            };

            let raw = core::str::from_utf8(&buf[..n]).unwrap();

            let mut headers = [httparse::EMPTY_HEADER; 16];
            let mut req = httparse::Request::new(&mut headers);
            if let Ok(res) = req.parse(raw.as_bytes()) {
                if res.is_complete() {
                    if let Some(path) = req.path {
                        if path.starts_with("/?code=") {
                            let mut code = path.trim_start_matches("/?code=");
                            if let Some(status_start) = code.find('&') {
                                code = &code[..status_start];
                            }
                            match socket.write_all(HTTP_RESP.as_bytes()).await {
                                Ok(()) => {}
                                Err(e) => {
                                    warn!("write error: {:?}", e);
                                    break;
                                }
                            }
                            socket.flush().await.unwrap();
                            out_buf[..code.len()].copy_from_slice(code.as_bytes());
                            return code.len();
                        } else if path.starts_with("/?error=") {
                            let mut error = path.trim_start_matches("/?error=");
                            if let Some(status_start) = error.find('&') {
                                error = &error[..status_start];
                            }
                            warn!("error: {}", error);
                        }
                    }
                }
            }
        }
    }
}

pub async fn get_access_token(
    seed: u64,
    tcp_client: &impl TcpConnect,
    dns: &impl Dns,
    auth_code: &str,
) -> (String<300>, String<300>) {
    let mut read_record_buffer = [0; 16640];
    let mut write_record_buffer = [0; 16640];

    let config = TlsConfig::new(
        seed,
        &mut read_record_buffer,
        &mut write_record_buffer,
        TlsVerify::None,
    );
    let mut client = HttpClient::new_with_tls(tcp_client, dns, config);

    let mut rx_buf = [0; 15360];

    let mut body = String::<512>::new();
    write!(
        body,
        "code={}&redirect_uri=http%3A%2F%2F10.0.0.41%3A3000&grant_type=authorization_code",
        auth_code
    )
    .unwrap();

    let mut request = client
        .request(Method::POST, "https://accounts.spotify.com/api/token/")
        .await
        .unwrap()
        .body(body.as_bytes())
        .headers(&[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Authorization", BASIC_AUTH),
        ]);

    let response = request.send(&mut rx_buf).await.unwrap();

    let body = response.body().read_to_end().await.unwrap();

    let (access_token_response, _) =
        serde_json_core::from_slice::<AccessTokenResponse>(body).unwrap();

    let mut access_token = String::<300>::new(); //Tokens do not have a set length, issues if it's greater than 200 bytes!
    let mut refresh_token = String::<300>::new(); //Tokens do not have a set length, issues if it's greater than 200 bytes!

    write!(
        access_token,
        "Bearer {}",
        access_token_response.access_token
    )
    .unwrap();
    write!(refresh_token, "{}", access_token_response.refresh_token).unwrap();

    (access_token, refresh_token)
}

pub async fn get_current_song(
    seed: u64,
    tcp_client: &impl TcpConnect,
    dns: &impl Dns,
    access_token: &str,
) -> (String<40>, String<40>) {
    let mut read_record_buffer = [0; 16640];
    let mut write_record_buffer = [0; 16640];

    let config = TlsConfig::new(
        seed,
        &mut read_record_buffer,
        &mut write_record_buffer,
        TlsVerify::None,
    );
    let mut client = HttpClient::new_with_tls(tcp_client, dns, config);

    let mut rx_buf = [0; 15360];

    let mut title = String::<40>::new();
    let mut artist = String::<40>::new();

    let headers = [("Authorization", access_token)];

    let mut request = client
        .request(
            Method::GET,
            "https://api.spotify.com/v1/me/player/currently-playing",
        )
        .await
        .unwrap()
        .headers(&headers);

    let response = request.send(&mut rx_buf).await.unwrap();

    let body = response.body().read_to_end().await.unwrap();

    let (current_playing_track, _) =
        serde_json_core::from_slice::<CurrentPlayingTrackResponse>(body).unwrap();

    for char in current_playing_track.item.name.chars().take(40) {
        write!(title, "{}", char).unwrap();
    }

    for char in current_playing_track.item.artists[0].name.chars().take(40) {
        write!(artist, "{}", char).unwrap();
    }

    (title, artist)
}
