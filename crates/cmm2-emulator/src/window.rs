//! The screen in an SDL window, scaled to fit with the picture's proportions kept.

use std::fmt;
use std::mem::{Discriminant, discriminant};
use std::time::{Duration, Instant};

use cmm2_core::{Frame, FrameError};
use sdl2::EventPump;
use sdl2::event::Event;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{Canvas, TextureCreator};
use sdl2::video::{Window as SdlWindow, WindowContext};

/// The window opens at this size; it can be resized.
const START_SIZE: (u32, u32) = (800, 600);

/// The window is redrawn at most this often, so the emulation does not spend its time on the
/// display. It is about 60 Hz.
const REDRAW: Duration = Duration::from_millis(16);

/// An SDL call failed; SDL reports its errors as text.
#[derive(Debug)]
pub struct WindowError(String);

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "window: {}", self.0)
    }
}

impl std::error::Error for WindowError {}

fn sdl<T, E: ToString>(result: Result<T, E>) -> Result<T, WindowError> {
    result.map_err(|err| WindowError(err.to_string()))
}

pub struct Window {
    canvas: Canvas<SdlWindow>,
    textures: TextureCreator<WindowContext>,
    events: EventPump,
    /// The last picture there was, which stays up while there is none.
    last: Option<Frame>,
    complaint: Complaint,
    drawn: Option<Instant>,
}

impl Window {
    pub fn open() -> Result<Window, WindowError> {
        let context = sdl(sdl2::init())?;
        let video = sdl(context.video())?;
        let window = sdl(video
            .window("Colour Maximite 2", START_SIZE.0, START_SIZE.1)
            .resizable()
            .position_centered()
            .build())?;
        let canvas = sdl(window.into_canvas().build())?;
        let textures = canvas.texture_creator();
        let events = sdl(context.event_pump())?;
        Ok(Window {
            canvas,
            textures,
            events,
            last: None,
            complaint: Complaint::default(),
            drawn: None,
        })
    }

    /// Once the redraw interval has passed, shows the frame the emulator has now, or the last
    /// good one if there is none, and handles the window's events. Returns false when the
    /// window has been closed. `frame` is only called when it is time to draw.
    pub fn update(
        &mut self,
        frame: impl FnOnce() -> Result<Frame, FrameError>,
    ) -> Result<bool, WindowError> {
        if self.drawn.is_some_and(|at| at.elapsed() < REDRAW) {
            return Ok(true);
        }
        self.drawn = Some(Instant::now());
        match frame() {
            Ok(frame) => {
                self.complaint.clear();
                self.last = Some(frame);
            }
            Err(err) => {
                if self.complaint.is_new(&err) {
                    eprintln!("cmm2-emulator: no picture: {err}");
                }
            }
        }
        self.draw()?;
        let open = !self
            .events
            .poll_iter()
            .any(|event| matches!(event, Event::Quit { .. }));
        Ok(open)
    }

    fn draw(&mut self) -> Result<(), WindowError> {
        self.canvas.set_draw_color(Color::BLACK);
        self.canvas.clear();
        if let Some(frame) = &self.last {
            let (width, height) = (frame.width as u32, frame.height as u32);
            // The texture is made for each draw, as it borrows the texture creator and the
            // picture can change size.
            let mut texture =
                sdl(self
                    .textures
                    .create_texture_streaming(PixelFormatEnum::RGB24, width, height))?;
            sdl(texture.update(None, &frame.rgb, 3 * frame.width))?;
            let (window_width, window_height) = sdl(self.canvas.output_size())?;
            let target = fit((window_width, window_height), (width, height));
            sdl(self.canvas.copy(&texture, None, target))?;
        }
        self.canvas.present();
        Ok(())
    }
}

/// The largest rectangle with the picture's proportions that fits in the window, centred.
fn fit(window: (u32, u32), picture: (u32, u32)) -> Rect {
    let (window_width, window_height) = (u64::from(window.0), u64::from(window.1));
    let (width, height) = (u64::from(picture.0), u64::from(picture.1));
    // Compare window_width / width with window_height / height, without dividing.
    let (fit_width, fit_height) = if window_width * height <= window_height * width {
        (window_width, window_width * height / width)
    } else {
        (window_height * width / height, window_height)
    };
    Rect::new(
        ((window_width - fit_width) / 2) as i32,
        ((window_height - fit_height) / 2) as i32,
        fit_width as u32,
        fit_height as u32,
    )
}

/// Remembers which kind of error was last reported, so that the same one is said only once.
#[derive(Default)]
struct Complaint(Option<Discriminant<FrameError>>);

impl Complaint {
    /// Whether this error is of a different kind than the one last reported.
    fn is_new(&mut self, err: &FrameError) -> bool {
        let kind = discriminant(err);
        let new = self.0 != Some(kind);
        self.0 = Some(kind);
        new
    }

    /// A picture came, so the next error is news again.
    fn clear(&mut self) {
        self.0 = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(rect: Rect) -> (i32, i32, u32, u32) {
        (rect.x(), rect.y(), rect.width(), rect.height())
    }

    #[test]
    fn the_picture_fills_a_window_of_its_own_shape() {
        assert_eq!(bounds(fit((800, 600), (800, 600))), (0, 0, 800, 600));
        assert_eq!(bounds(fit((1600, 1200), (800, 600))), (0, 0, 1600, 1200));
    }

    #[test]
    fn a_wide_window_has_bars_at_the_sides() {
        assert_eq!(bounds(fit((1000, 600), (800, 600))), (100, 0, 800, 600));
    }

    #[test]
    fn a_tall_window_has_bars_above_and_below() {
        assert_eq!(bounds(fit((800, 900), (800, 600))), (0, 150, 800, 600));
    }

    #[test]
    fn a_window_smaller_than_the_picture_shrinks_it() {
        assert_eq!(bounds(fit((400, 400), (800, 600))), (0, 50, 400, 300));
    }

    #[test]
    fn an_error_is_new_until_it_repeats_or_a_picture_comes() {
        let mut complaint = Complaint::default();
        assert!(complaint.is_new(&FrameError::Off));
        assert!(!complaint.is_new(&FrameError::Off));
        assert!(complaint.is_new(&FrameError::Unreadable(0x10)));
        assert!(!complaint.is_new(&FrameError::Unreadable(0x20)));
        complaint.clear();
        assert!(complaint.is_new(&FrameError::Unreadable(0x20)));
    }
}
