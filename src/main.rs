#![no_std]
#![no_main]

mod access_token_response;
mod current_playing_track;
mod spotify;

use crate::spotify::{get_access_token, get_current_song, get_spotify_code};
use cyw43_pio::PioSpi;
use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Config, Stack, StackResources};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::{bind_interrupts, i2c};
use embassy_time::{Delay, Timer};
use embedded_hal::delay::DelayNs;
use heapless::String;
use panic_probe as _;
use static_cell::StaticCell;

const LCD_ADDR: u8 = 0x27;
const FUNCTION_SET: u8 = 0x20;
const TWO_LINE_DISPLAY: u8 = 0x8;
const BACKLIGHT: u8 = 0x8;
const EIGHT_BIT_MODE: u8 = 0x10;

const DISPLAY_COMMAND: u8 = 0x8;
const DISPLAY_ON: u8 = 0x4;
const _CURSOR_ON: u8 = 0x2;
const _BLINK_ON: u8 = 0x1;
const ENTRY_MODE_COMMAND: u8 = 0x4;
const ENTRY_RIGHT: u8 = 0x2;

const CLEAR_SCREEN: u8 = 0x1;
const SHIFT_CURSOR: u8 = 16 | 4;
const RETURN_HOME: u8 = 0x2;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

const WIFI_NETWORK: &str = env!("WIFI_NETWORK");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

const BASIC_AUTH: &str = env!("SPOTIFY_BASIC_AUTH");

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

static FW: &[u8] = include_bytes!("../cyw43-firm/43439A0.bin");
static CLM: &[u8] = include_bytes!("../cyw43-firm/43439A0_clm.bin");

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let sda = p.PIN_0;
    let slc = p.PIN_1;

    let mut i2c = i2c::I2c::new_blocking(p.I2C0, slc, sda, Default::default());

    Delay.delay_ms(80);

    write4bits(&mut i2c, FUNCTION_SET | EIGHT_BIT_MODE);
    Delay.delay_ms(5);
    write4bits(&mut i2c, FUNCTION_SET | EIGHT_BIT_MODE);
    Delay.delay_ms(5);
    write4bits(&mut i2c, FUNCTION_SET | EIGHT_BIT_MODE);
    Delay.delay_ms(5);

    write4bits(&mut i2c, FUNCTION_SET);

    command(&mut i2c, FUNCTION_SET | TWO_LINE_DISPLAY);
    command(&mut i2c, DISPLAY_COMMAND | DISPLAY_ON);
    command(&mut i2c, CLEAR_SCREEN);
    command(&mut i2c, ENTRY_MODE_COMMAND | ENTRY_RIGHT);

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();

    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, FW).await;

    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(CLM).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());

    let seed = 0x0123_4567_89AB_CDEF;

    static STACK: StaticCell<Stack<cyw43::NetDriver<'static>>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        net_device,
        config,
        RESOURCES.init(StackResources::<4>::new()),
        seed,
    ));

    unwrap!(spawner.spawn(net_task(stack)));

    loop {
        match control.join_wpa2(WIFI_NETWORK, WIFI_PASSWORD).await {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    info!("waiting for DHCP...");
    stack.wait_config_up().await;
    info!("DHCP is now up!");

    let mut auth_code_buf = [0; 256];
    let auth_code_len = get_spotify_code(stack, &mut auth_code_buf).await;
    let auth_code = core::str::from_utf8(&auth_code_buf[..auth_code_len]).unwrap();

    let client_state = TcpClientState::<1, 16640, 16640>::new();
    let tcp_client = TcpClient::new(stack, &client_state);

    let dns = DnsSocket::new(stack);

    let (access_token, _refresh_token) = get_access_token(seed, &tcp_client, &dns, auth_code).await;

    let mut old_title = String::<40>::new();
    let mut old_artist = String::<40>::new();
    loop {
        let (title, artist) = get_current_song(seed, &tcp_client, &dns, &access_token).await;

        if old_title == title && old_artist == artist {
            Timer::after_secs(5).await;
            continue;
        }

        command(&mut i2c, RETURN_HOME);
        Delay.delay_ms(2);
        command(&mut i2c, CLEAR_SCREEN);
        Delay.delay_ms(2);

        for b in title.as_bytes() {
            write(&mut i2c, *b);
        }

        set_cursor(&mut i2c, 1, 0);

        for b in artist.as_bytes() {
            write(&mut i2c, *b);
        }

        old_title = title;
        old_artist = artist;

        Timer::after_secs(5).await;
    }
}

fn write4bits<I2C: embedded_hal::i2c::I2c>(i2c: &mut I2C, data: u8) {
    i2c.write(LCD_ADDR, &[data | 0x4 | BACKLIGHT]).unwrap();
    Delay.delay_ms(5);
    i2c.write(LCD_ADDR, &[BACKLIGHT]).unwrap();
    Delay.delay_ms(5);
}

fn send<I2C: embedded_hal::i2c::I2c>(i2c: &mut I2C, data: u8, mode: u8) {
    let high_bits = data & 0xF0;
    let low_bits = (data << 4) & 0xF0;
    write4bits(i2c, high_bits | mode);
    write4bits(i2c, low_bits | mode);
}

fn write<I2C: embedded_hal::i2c::I2c>(i2c: &mut I2C, data: u8) {
    send(i2c, data, 0x1);
}

fn command<I2C: embedded_hal::i2c::I2c>(i2c: &mut I2C, data: u8) {
    send(i2c, data, 0x0);
}

fn set_cursor<I2C: embedded_hal::i2c::I2c>(i2c: &mut I2C, row: u8, col: u8) {
    command(i2c, RETURN_HOME);
    Delay.delay_ms(2);
    let shift: u8 = row * 40 + col;
    for _ in 0..shift {
        command(i2c, SHIFT_CURSOR);
    }
}
