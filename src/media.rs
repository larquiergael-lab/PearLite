//! Windows System Media Transport Controls: media keys, the volume/lock-screen
//! media flyout with artwork, and Bluetooth headset buttons.

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
}

pub struct Smtc(MediaControls);

impl Smtc {
    pub fn new(hwnd: isize, on_event: impl Fn(MediaControlEvent) + Send + 'static) -> Result<Self, souvlaki::Error> {
        let mut controls = MediaControls::new(PlatformConfig {
            display_name: "Pear Lite",
            dbus_name: "pear_lite",
            hwnd: Some(hwnd as *mut std::ffi::c_void),
        })?;
        controls.attach(on_event)?;
        Ok(Self(controls))
    }

    pub fn set_track(&mut self, track: &Track, duration: f64) {
        let _ = self.0.set_metadata(MediaMetadata {
            title: Some(&track.title),
            artist: Some(&track.artist),
            album: Some(&track.album),
            cover_url: (!track.art.is_empty()).then_some(track.art.as_str()),
            duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
        });
    }

    pub fn set_playback(&mut self, playing: bool, position: f64) {
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let _ = self.0.set_playback(if playing {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        });
    }
}
