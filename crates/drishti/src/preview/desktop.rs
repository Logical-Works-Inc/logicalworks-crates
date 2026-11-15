//! Optional native read-only window: CPU reference presentation.
//!
//! Absorbed from `primitives/canvas-native/crates/preview/src/desktop.rs`.
//! Not a GPU-efficiency claim. Available only with the `desktop` feature.

use lgwks_deps::softbuffer;
use lgwks_deps::winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};
use std::{
    num::NonZeroU32,
    rc::Rc,
    time::{Duration, Instant},
};

use super::Failure;
use super::paint::Painter;
use crate::canvas_scene::{Action, Canvas, Frame, View};

/// Window plus its CPU surface.
struct Display {
    /// CPU surface.
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// Window handle.
    window: Rc<Window>,
}

/// Application state for the event loop.
struct App {
    /// Presentation instance.
    canvas: Canvas,
    /// Window, once resumed.
    display: Option<Display>,
    /// Reference painter.
    painter: Painter,
    /// Currently visible frame, if any.
    visible: Option<Frame>,
    /// Last cursor position.
    cursor: (f64, f64),
    /// Last render time.
    last: Instant,
    /// Next animation tick, if motion is unsettled.
    next_tick: Option<Instant>,
    /// Whether the window is occluded.
    hidden: bool,
    /// Terminal failure, if the loop must exit with an error.
    failure: Option<Failure>,
}

impl App {
    /// Record a terminal failure, revoke the instance, and exit the loop.
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Failure) {
        self.failure = Some(error);
        self.canvas.revoke();
        self.next_tick = None;
        event_loop.exit();
    }

    /// Request a redraw unless occluded.
    fn redraw(&mut self) {
        if self.hidden {
            return;
        }
        if let Some(ref display) = self.display {
            display.window.request_redraw();
        }
    }

    /// Route a local interaction against the visible frame's token.
    fn action(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        if let Some(ref frame) = self.visible {
            if let Err(error) = self.canvas.input(frame.token(), action) {
                if let Err(report) = report_rejection(&format!("rejected input: {error}")) {
                    self.fail(event_loop, report);
                    return;
                }
                self.redraw();
                return;
            }
            // A new target begins now, not at the last repaint before an idle
            // interval. Resume from the displayed pose/velocity; do not
            // integrate idle time into a new command.
            self.last = Instant::now();
            self.redraw();
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    /// Render one frame into the window surface.
    fn render(&mut self) -> Result<(), Failure> {
        let Some(ref mut display) = self.display else {
            return Ok(());
        };
        let size = display.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            self.next_tick = None;
            return Ok(());
        };
        self.canvas.limits().pixel_count(size.width, size.height)?;
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        if elapsed > 60.0 {
            self.canvas.resume_without_replay();
        }
        let frame = self
            .canvas
            .prepare(
                if elapsed > 60.0 { 0.0 } else { elapsed },
                size.width,
                size.height,
                true,
            )?
            .ok_or("redraw frame absent")?;
        display.surface.resize(width, height)?;
        let mut pixels = display.surface.buffer_mut()?;
        self.painter.draw(&frame, &mut pixels)?;
        pixels.present()?;
        self.canvas.acknowledge_present(frame.token())?;
        display.window.set_title(&format!(
            "Canvas | {} | status: {} | read-only, no domain integration",
            frame.target().label(),
            frame.status_label()
        ));
        self.next_tick = if frame.animating() {
            Instant::now().checked_add(Duration::from_millis(16))
        } else {
            None
        };
        self.visible = Some(frame);
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.display.is_some() {
            return;
        }
        let create = || -> Result<Display, Failure> {
            let window = Rc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Canvas | read-only source preview")
                        .with_inner_size(LogicalSize::new(960.0, 640.0))
                        .with_min_inner_size(LogicalSize::new(480.0, 320.0)),
                )?,
            );
            let context = softbuffer::Context::new(Rc::clone(&window))?;
            let surface = softbuffer::Surface::new(&context, Rc::clone(&window))?;
            Ok(Display { surface, window })
        };
        match create() {
            Ok(display) => {
                self.display = Some(display);
                self.hidden = false;
                self.last = Instant::now();
                self.canvas.resume_without_replay();
                self.redraw();
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.visible = None;
        self.display = None;
        self.next_tick = None;
        self.canvas.invalidate_surface();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.display.as_ref().map(|held| held.window.id()) != Some(id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.canvas.revoke();
                self.next_tick = None;
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.visible = None;
                self.canvas.invalidate_surface();
                self.redraw();
            }
            WindowEvent::Occluded(hidden) => {
                self.hidden = hidden;
                self.next_tick = None;
                if !hidden {
                    self.last = Instant::now();
                    self.canvas.resume_without_replay();
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested if !self.hidden => {
                if let Err(error) = self.render() {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = (position.x, position.y),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(view) = self
                    .visible
                    .as_ref()
                    .and_then(|shown| shown.hit_test(self.cursor.0, self.cursor.1))
                {
                    self.action(event_loop, Action::Navigate(view));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, vertical) => {
                        scroll_lines(f64::from(-vertical * 3.0))
                    }
                    MouseScrollDelta::PixelDelta(position) => scroll_lines(-position.y / 18.0),
                };
                self.action(event_loop, Action::scroll(self.canvas.target(), lines));
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let action = match event.logical_key {
                    Key::Named(NamedKey::Escape) => Some(Action::Navigate(View::Source)),
                    Key::Named(NamedKey::ArrowRight | NamedKey::Tab) => Some(Action::Step(1)),
                    Key::Named(NamedKey::ArrowLeft) => Some(Action::Step(-1)),
                    Key::Named(NamedKey::ArrowDown) => {
                        Some(Action::scroll(self.canvas.target(), 3))
                    }
                    Key::Named(NamedKey::ArrowUp) => Some(Action::scroll(self.canvas.target(), -3)),
                    Key::Character(ref text) if text.eq_ignore_ascii_case("r") && !event.repeat => {
                        Some(Action::ToggleReducedMotion)
                    }
                    Key::Character(ref text) if text == "1" => Some(Action::Navigate(View::Source)),
                    Key::Character(ref text) if text == "2" => {
                        Some(Action::Navigate(View::Proposal))
                    }
                    Key::Character(ref text) if text == "3" => {
                        Some(Action::Navigate(View::Evidence))
                    }
                    _ => None,
                };
                if let Some(action) = action {
                    self.action(event_loop, action);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.hidden {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if let Some(deadline) = self.next_tick {
            if Instant::now() >= deadline {
                self.next_tick = None;
                self.redraw();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
                return;
            }
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

/// Open the reference window and run it to close.
///
/// # Errors
///
/// Returns window, surface, or presentation failures.
///
/// # Examples
///
/// Opening a window needs a display, so the example pins the contract the
/// `canvas_preview` binary relies on: the canvas moves in and a typed
/// [`Failure`] comes out.
///
/// ```
/// use drishti::canvas_scene::Canvas;
/// use drishti::cli::Failure;
/// use drishti::preview::desktop::run_window;
///
/// let entry: fn(Canvas) -> Result<(), Failure> = run_window;
/// assert_eq!(std::mem::size_of_val(&entry), std::mem::size_of::<usize>());
/// ```
pub fn run_window(canvas: Canvas) -> Result<(), Failure> {
    let event_loop = EventLoop::new()?;
    let mut app = App {
        canvas,
        display: None,
        painter: Painter::new(),
        visible: None,
        cursor: (0.0, 0.0),
        last: Instant::now(),
        next_tick: None,
        hidden: false,
        failure: None,
    };
    event_loop.run_app(&mut app)?;
    match app.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Report a rejected input to stderr; a failed report fails the loop through
/// [`Failure`], so an unwritten rejection can never pass silently.
///
/// # Errors
///
/// Returns the write failure when stderr cannot record the rejection.
fn report_rejection(detail: &str) -> Result<(), Failure> {
    use std::io::Write as _;
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    writeln!(handle, "canvas-preview: {detail}")
        .map_err(|error| Failure::from(format!("the rejection report was not written: {error}")))?;
    Ok(())
}

/// Truncate a wheel delta to whole lines in [-100, 100] without an `as`
/// cast. NaN maps to zero, matching `as` saturation.
fn scroll_lines(value: f64) -> i32 {
    let clamped = value.clamp(-100.0, 100.0);
    if !clamped.is_finite() {
        return 0;
    }
    let negative = clamped < 0.0;
    let mut magnitude = if negative { -clamped } else { clamped };
    let mut whole: i32 = 0;
    while magnitude >= 1.0 {
        magnitude -= 1.0;
        whole = whole.wrapping_add(1);
    }
    if negative {
        whole.saturating_neg()
    } else {
        whole
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_lines;

    /// Wheel deltas truncate exactly, clamp at the rails, and map NaN to zero.
    #[test]
    fn scroll_deltas_truncate_without_casts() -> Result<(), String> {
        if scroll_lines(2.7) != 2 {
            return Err(String::from("positive delta must truncate"));
        }
        if scroll_lines(-2.7) != -2 {
            return Err(String::from("negative delta must truncate toward zero"));
        }
        if scroll_lines(1000.0) != 100 {
            return Err(String::from("large delta must clamp"));
        }
        if scroll_lines(-1000.0) != -100 {
            return Err(String::from("large negative delta must clamp"));
        }
        if scroll_lines(f64::NAN) != 0 {
            return Err(String::from("NaN must map to zero"));
        }
        Ok(())
    }
}
