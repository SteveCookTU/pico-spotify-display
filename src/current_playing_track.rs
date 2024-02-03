use heapless::Vec;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CurrentPlayingTrackResponse<'a> {
    #[serde(borrow)]
    pub item: CurrentPlayingTrackItem<'a>,
}

#[derive(Deserialize)]
pub struct CurrentPlayingTrackItem<'a> {
    pub name: &'a str,
    #[serde(borrow)]
    pub artists: Vec<CurrentPlayingTrackArtist<'a>, 6>,
}

#[derive(Deserialize, Default, Copy, Clone)]
pub struct CurrentPlayingTrackArtist<'a> {
    pub name: &'a str,
}
