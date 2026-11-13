//! The screen in an SDL window, scaled to fit with the picture's proportions kept, and its
//! keyboard as the guest's: the USB keyboard's report once the firmware polls it, and until then
//! (or with no USB keys) the console's input.

use std::fmt;
use std::mem::{Discriminant, discriminant};
use std::time::{Duration, Instant};

use cmm2_core::{Frame, FrameError};
use sdl2::EventPump;
use sdl2::event::{Event, WindowEvent};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{Canvas, TextureCreator};
use sdl2::video::{Window as SdlWindow, WindowContext};

use crate::keys::{key_bytes, text_bytes};
use crate::usb_keys::{UsbKeys, usage};

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
    /// The USB keyboard's keys, if the window drives it.
    usb: Option<UsbKeys>,
}

impl Window {
    /// With `usb`, the keys go to the USB keyboard as soon as the firmware polls it; without,
    /// they are always the console's.
    pub fn open(usb: Option<UsbKeys>) -> Result<Window, WindowError> {
        let context = sdl(sdl2::init())?;
        let video = sdl(context.video())?;
        // Printable keys arrive as text, which has the layout and shift applied.
        video.text_input().start();
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
            usb,
        })
    }

    /// Once the redraw interval has passed, shows the frame the emulator has now, or the last
    /// good one if there is none, and handles the window's events: the console bytes for
    /// the keys that are the console's are passed to `input` as bytes, and the others set the USB
    /// keyboard's report. Returns false when the window has been closed.
    /// `frame` is only called when it is time to draw.
    pub fn update(
        &mut self,
        mut input: impl FnMut(&[u8]),
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
        if let Some(usb) = &mut self.usb {
            usb.sync();
        }
        // Whether the keys are USB's is settled for the whole batch: the firmware cannot start
        // polling while the events are handled.
        let to_usb = self.usb.as_ref().is_some_and(UsbKeys::active);
        let mut open = true;
        for event in self.events.poll_iter() {
            match event {
                Event::Quit { .. } => open = false,
                Event::KeyDown {
                    keycode,
                    scancode,
                    keymod,
                    repeat,
                    ..
                } => {
                    // The firmware repeats a key itself, so the report ignores SDL's repeats.
                    if !repeat
                        && let (Some(usb), Some(usage)) = (&mut self.usb, scancode.and_then(usage))
                    {
                        usb.press(usage);
                    }
                    if !to_usb && let Some(bytes) = keycode.and_then(|k| key_bytes(k, keymod)) {
                        input(bytes);
                    }
                }
                Event::KeyUp { scancode, .. } => {
                    if let (Some(usb), Some(usage)) = (&mut self.usb, scancode.and_then(usage)) {
                        usb.release(usage);
                    }
                }
                Event::TextInput { text, .. } if !to_usb => input(&text_bytes(&text)),
                // The releases will not come to a window that has lost the focus.
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                } => {
                    if let Some(usb) = &mut self.usb {
                        usb.release_all();
                    }
                }
                _ => {}
            }
        }
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
    use cmm2_core::{HidKeyboard, KeyReport, UsbDevice};
    use sdl2::keyboard::Scancode;
    use sdl2::keyboard::{Keycode, Mod};
    use sdl2::sys::{
        SDL_Event, SDL_EventType, SDL_KeyboardEvent, SDL_Keysym, SDL_PRESSED, SDL_PushEvent,
        SDL_RELEASED, SDL_Scancode, SDL_TextInputEvent, SDL_WindowEvent, SDL_WindowEventID,
    };

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

    // SDL's own queue is the only way in, as the Rust wrapper has no way to make a text event.
    fn push_text(text: &str) {
        let mut bytes = [0; 32];
        for (slot, byte) in bytes.iter_mut().zip(text.bytes()) {
            *slot = byte as _;
        }
        let mut event = SDL_Event {
            text: SDL_TextInputEvent {
                type_: SDL_EventType::SDL_TEXTINPUT as u32,
                timestamp: 0,
                windowID: 0,
                text: bytes,
            },
        };
        // SAFETY: the pointer is to a live event, which SDL copies.
        assert_eq!(unsafe { SDL_PushEvent(&mut event) }, 1);
    }

    fn push_key(
        keycode: Keycode,
        scancode: Option<Scancode>,
        keymod: Mod,
        down: bool,
        repeat: bool,
    ) {
        // SAFETY: SDL's scancode enum has the numbers the wrapper's Scancode has.
        let scancode = scancode.map_or(SDL_Scancode::SDL_SCANCODE_UNKNOWN, |scancode| unsafe {
            std::mem::transmute::<u32, SDL_Scancode>(scancode as u32)
        });
        let mut event = SDL_Event {
            key: SDL_KeyboardEvent {
                type_: if down {
                    SDL_EventType::SDL_KEYDOWN
                } else {
                    SDL_EventType::SDL_KEYUP
                } as u32,
                timestamp: 0,
                windowID: 0,
                state: if down { SDL_PRESSED } else { SDL_RELEASED } as u8,
                repeat: u8::from(repeat),
                padding2: 0,
                padding3: 0,
                keysym: SDL_Keysym {
                    scancode,
                    sym: i32::from(keycode),
                    mod_: keymod.bits(),
                    unused: 0,
                },
            },
        };
        // SAFETY: the pointer is to a live event, which SDL copies.
        assert_eq!(unsafe { SDL_PushEvent(&mut event) }, 1);
    }

    // A key with nothing but its keycode, as the console path only needs that.
    fn push_keycode(keycode: Keycode, keymod: Mod) {
        push_key(keycode, None, keymod, true, false);
    }

    fn push_focus_lost() {
        let mut event = SDL_Event {
            window: SDL_WindowEvent {
                type_: SDL_EventType::SDL_WINDOWEVENT as u32,
                timestamp: 0,
                windowID: 0,
                event: SDL_WindowEventID::SDL_WINDOWEVENT_FOCUS_LOST as u8,
                padding1: 0,
                padding2: 0,
                padding3: 0,
                data1: 0,
                data2: 0,
            },
        };
        // SAFETY: the pointer is to a live event, which SDL copies.
        assert_eq!(unsafe { SDL_PushEvent(&mut event) }, 1);
    }

    // The time that has passed since the last update, as far as the redraw interval goes.
    fn update(window: &mut Window, typed: &mut Vec<u8>) -> bool {
        window.drawn = None;
        window
            .update(|bytes| typed.extend(bytes), || Err(FrameError::Off))
            .unwrap()
    }

    // SDL allows one instance in a process, so the window is opened in this one test. It has the
    // USB keys, and the keyboard they drive is not polled yet.
    #[test]
    fn the_windows_keys_reach_the_console_then_the_usb_keyboard_and_closing_it_ends_the_run() {
        assert!(sdl2::hint::set("SDL_VIDEODRIVER", "dummy"));
        let report = KeyReport::new();
        let mut keyboard = HidKeyboard::new(report.clone());
        keyboard.setup([0x00, 0x09, 1, 0, 0, 0, 0, 0]);
        keyboard.in_token(0, 8);
        let mut window = Window::open(Some(UsbKeys::new(report.clone()))).unwrap();
        let mut typed: Vec<u8> = Vec::new();

        // Before the firmware polls, the keys are the console's.
        push_text("a");
        push_keycode(Keycode::Left, Mod::NOMOD);
        push_keycode(Keycode::C, Mod::LCTRLMOD);
        push_text("é");
        push_keycode(Keycode::Return, Mod::NOMOD);
        push_keycode(Keycode::LShift, Mod::NOMOD);
        assert!(update(&mut window, &mut typed));
        assert_eq!(typed, b"a\x1b[D\x03\r");
        assert_eq!(report.get(), [0; 8]);

        // The modifiers are kept for the report all the while, other keys are not.
        push_key(
            Keycode::LShift,
            Some(Scancode::LShift),
            Mod::LSHIFTMOD,
            true,
            false,
        );
        push_key(Keycode::B, Some(Scancode::B), Mod::LSHIFTMOD, true, false);
        assert!(update(&mut window, &mut typed));
        assert_eq!(report.get(), [2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(typed, b"a\x1b[D\x03\r");

        // Within the redraw interval nothing is handled, and the keys wait for the next time.
        push_text("b");
        window
            .update(|bytes| typed.extend(bytes), || Err(FrameError::Off))
            .unwrap();
        assert_eq!(typed, b"a\x1b[D\x03\r");
        assert!(update(&mut window, &mut typed));
        assert_eq!(typed, b"a\x1b[D\x03\rb");

        // The firmware polls: from the next batch the keys are USB's and no longer the
        // console's.
        keyboard.in_token(1, 8);
        push_text("c");
        push_keycode(Keycode::Left, Mod::NOMOD);
        push_key(Keycode::A, Some(Scancode::A), Mod::LSHIFTMOD, true, false);
        push_key(Keycode::A, Some(Scancode::A), Mod::LSHIFTMOD, true, true);
        push_key(
            Keycode::Left,
            Some(Scancode::Left),
            Mod::LSHIFTMOD,
            true,
            false,
        );
        // B was held across the switch, and its repeats are not presses.
        push_key(Keycode::B, Some(Scancode::B), Mod::LSHIFTMOD, true, true);
        assert!(update(&mut window, &mut typed));
        assert_eq!(typed, b"a\x1b[D\x03\rb");
        assert_eq!(report.get(), [2, 0, 0x04, 0x50, 0, 0, 0, 0]);

        // A release changes the report, and a key held across the switch (B) has no effect.
        keyboard.in_token(1, 8);
        push_key(Keycode::A, Some(Scancode::A), Mod::LSHIFTMOD, false, false);
        push_key(Keycode::B, Some(Scancode::B), Mod::LSHIFTMOD, false, false);
        assert!(update(&mut window, &mut typed));
        assert_eq!(report.get(), [2, 0, 0x50, 0, 0, 0, 0, 0]);

        // A key tapped between two polls is in the report until the firmware has it, and the
        // next look, with no event, lets it go.
        push_key(Keycode::C, Some(Scancode::C), Mod::LSHIFTMOD, true, false);
        push_key(Keycode::C, Some(Scancode::C), Mod::LSHIFTMOD, false, false);
        assert!(update(&mut window, &mut typed));
        assert_eq!(report.get(), [2, 0, 0x50, 0x06, 0, 0, 0, 0]);
        keyboard.in_token(1, 8);
        assert!(update(&mut window, &mut typed));
        assert_eq!(report.get(), [2, 0, 0x50, 0, 0, 0, 0, 0]);

        // Losing the focus lets go of everything.
        push_focus_lost();
        assert!(update(&mut window, &mut typed));
        assert_eq!(report.get(), [0; 8]);
        assert_eq!(typed, b"a\x1b[D\x03\rb");

        // Without USB keys the keys are the console's whatever the firmware does.
        window.usb = None;
        push_text("d");
        push_keycode(Keycode::Left, Mod::NOMOD);
        assert!(update(&mut window, &mut typed));
        assert_eq!(typed, b"a\x1b[D\x03\rbd\x1b[D");

        let mut event = SDL_Event {
            type_: SDL_EventType::SDL_QUIT as u32,
        };
        // SAFETY: the pointer is to a live event, which SDL copies.
        assert_eq!(unsafe { SDL_PushEvent(&mut event) }, 1);
        assert!(!update(&mut window, &mut typed));
    }
}
