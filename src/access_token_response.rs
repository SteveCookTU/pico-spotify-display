use serde::Deserialize;

#[derive(Deserialize)]
pub struct AccessTokenResponse<'a> {
    pub access_token: &'a str,
    pub token_type: &'a str,
    pub scope: &'a str,
    pub expires_in: u64,
    pub refresh_token: &'a str,
}
