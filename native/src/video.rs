use crate::header::{action_button, pill, spinner};
use futures::{StreamExt, channel::mpsc};
use gpui::{prelude::*, *};
use std::{
    cell::Cell,
    ffi::{CString, c_char, c_int, c_void},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self as commands, SyncSender},
    },
    thread,
    time::Duration,
};

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct Event {
    kind: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}
pub struct Fullscreen(pub bool);
#[derive(Clone, Copy, PartialEq)]
enum Slider {
    Seek,
    Volume,
}
#[derive(Clone, Copy)]
struct Drag {
    slider: Slider,
    ratio: f64,
}

struct Api {
    create: unsafe extern "C" fn() -> *mut c_void,
    init: unsafe extern "C" fn(*mut c_void) -> c_int,
    option: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(*mut c_void, u64, *const *const c_char) -> c_int,
    event: unsafe extern "C" fn(*mut c_void, f64) -> *const Event,
    observe: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    terminate: unsafe extern "C" fn(*mut c_void),
    context: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut Param) -> c_int,
    callback:
        unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    update: unsafe extern "C" fn(*mut c_void) -> u64,
    render: unsafe extern "C" fn(*mut c_void, *mut Param) -> c_int,
    free: unsafe extern "C" fn(*mut c_void),
    _library: libloading::Library,
}
impl Api {
    fn load(cache: PathBuf) -> Result<Arc<Self>, String> {
        static API: OnceLock<Arc<Api>> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api.clone());
        }
        let api = Arc::new(Self::open(cache)?);
        // Keep codec libraries loaded until process exit; their TLS can outlive a player.
        let _ = API.set(api.clone());
        Ok(API.get().unwrap().clone())
    }
    fn open(cache: PathBuf) -> Result<Self, String> {
        let names = if cfg!(windows) {
            vec!["mpv-2.dll", "libmpv-2.dll"]
        } else if cfg!(target_os = "macos") {
            vec!["libmpv.2.dylib"]
        } else {
            vec!["libmpv.so.2"]
        };
        let paths = std::env::var_os("RAESIR_MPV_LIBRARY")
            .map(PathBuf::from)
            .into_iter()
            .chain(names.iter().map(|s| PathBuf::from(*s)))
            .chain(names.iter().map(|s| cache.join("runtime").join(s)));
        let library = paths
            .filter_map(|path| unsafe { libloading::Library::new(path).ok() })
            .next()
            .ok_or(
                "Inline playback needs libmpv. You can still open the trailer in your player.",
            )?;
        unsafe {
            macro_rules! symbol {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| e.to_string())?
                };
            }
            Ok(Self {
                create: symbol!("mpv_create"),
                init: symbol!("mpv_initialize"),
                option: symbol!("mpv_set_option_string"),
                command: symbol!("mpv_command_async"),
                event: symbol!("mpv_wait_event"),
                observe: symbol!("mpv_observe_property"),
                terminate: symbol!("mpv_terminate_destroy"),
                context: symbol!("mpv_render_context_create"),
                callback: symbol!("mpv_render_context_set_update_callback"),
                update: symbol!("mpv_render_context_update"),
                render: symbol!("mpv_render_context_render"),
                free: symbol!("mpv_render_context_free"),
                _library: library,
            })
        }
    }
}
#[derive(Default)]
struct Playback {
    frame: Option<Arc<RenderImage>>,
    error: Option<String>,
    position: f64,
    duration: f64,
    volume: f64,
    paused: bool,
    muted: bool,
}
pub struct Player {
    url: String,
    state: Arc<Mutex<Playback>>,
    commands: commands::Sender<Vec<String>>,
    stop: Arc<AtomicBool>,
    fullscreen: bool,
    restore_window: bool,
    drag: Option<Drag>,
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    last_frame: Option<Arc<RenderImage>>,
    cleanup_registered: bool,
    _task: Task<()>,
}
impl Player {
    pub fn new(url: String, cache: PathBuf, cx: &mut Context<Self>) -> Self {
        let state = Arc::new(Mutex::new(Playback {
            volume: 100.,
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = commands::channel();
        let (mut notify, mut receive) = mpsc::channel::<()>(1);
        let task = cx.spawn(async move |this, cx| {
            while receive.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let thread_state = state.clone();
        let thread_stop = stop.clone();
        let source = url.clone();
        thread::spawn(move || {
            let result = run(
                source,
                cache,
                &thread_state,
                &thread_stop,
                command_rx,
                &mut notify,
            );
            if let Err(error) = result {
                thread_state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(error);
                let _ = notify.try_send(());
            }
        });
        Self {
            url,
            state,
            commands: command_tx,
            stop,
            fullscreen: false,
            restore_window: false,
            drag: None,
            seek_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            last_frame: None,
            cleanup_registered: false,
            _task: task,
        }
    }
    fn command(&self, args: &[&str]) {
        let _ = self
            .commands
            .send(args.iter().map(|s| s.to_string()).collect());
    }
}
impl EventEmitter<Fullscreen> for Player {}
impl Player {
    fn toggle_pause(&self) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.duration > 0. && state.position >= state.duration - 0.2 {
            drop(state);
            self.command(&["seek", "0", "absolute"]);
            self.command(&["set", "pause", "no"]);
        } else {
            drop(state);
            self.command(&["cycle", "pause"]);
        }
    }
    pub fn leave_fullscreen(&mut self, window: &mut Window) {
        if self.restore_window && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.restore_window = false;
        self.fullscreen = false;
        self.drag = None;
    }
    fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fullscreen {
            self.leave_fullscreen(window);
        } else {
            self.restore_window = !window.is_fullscreen();
            if self.restore_window {
                window.toggle_fullscreen();
            }
            self.fullscreen = true;
        }
        cx.emit(Fullscreen(self.fullscreen));
        cx.notify();
    }
    fn drag_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let bounds = if drag.slider == Slider::Seek {
            self.seek_bounds.get()
        } else {
            self.volume_bounds.get()
        };
        let width = f32::from(bounds.size.width);
        if width > 0. {
            drag.ratio = ((f32::from(position.x - bounds.origin.x) / width) as f64).clamp(0., 1.);
            cx.notify();
        }
    }
    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            match drag.slider {
                Slider::Seek => {
                    let duration = self
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .duration;
                    self.command(&["seek", &format!("{}", drag.ratio * duration), "absolute"]);
                }
                Slider::Volume => {
                    self.command(&["set", "volume", &format!("{}", drag.ratio * 100.)]);
                    if drag.ratio > 0. {
                        self.command(&["set", "mute", "no"]);
                    }
                }
            }
            cx.notify();
        }
    }
    fn slider(
        &self,
        id: &'static str,
        slider: Slider,
        ratio: f64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let bounds = if slider == Slider::Seek {
            self.seek_bounds.clone()
        } else {
            self.volume_bounds.clone()
        };
        div()
            .id(id)
            .relative()
            .h(rems(0.9))
            .cursor_pointer()
            .on_prepaint(move |paint, _, _| bounds.set(paint.bounds))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(rems(0.325))
                    .h(rems(0.25))
                    .rounded_full()
                    .bg(rgba(0xffffff45))
                    .child(
                        div()
                            .h_full()
                            .w(relative(ratio as f32))
                            .rounded_full()
                            .bg(rgb(0xe2f0e8)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(ratio as f32))
                    .ml(rems(-0.35))
                    .top(rems(0.1))
                    .size(rems(0.7))
                    .rounded_full()
                    .bg(rgb(0xf5fbf6)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.drag = Some(Drag { slider, ratio: 0. });
                    this.drag_at(event.position, cx);
                }),
            )
    }
}
fn timestamp(seconds: f64) -> String {
    let value = if seconds.is_finite() {
        seconds.max(0.) as u64
    } else {
        0
    };
    if value >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            value / 3600,
            (value / 60) % 60,
            value % 60
        )
    } else {
        format!("{}:{:02}", value / 60, value % 60)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
unsafe extern "C" fn wake(data: *mut c_void) {
    // mpv owns no Rust data; the sender stays alive until the render context is freed.
    let sender = unsafe { &*(data as *const SyncSender<()>) };
    let _ = sender.try_send(());
}
fn run(
    url: String,
    cache: PathBuf,
    state: &Arc<Mutex<Playback>>,
    stop: &AtomicBool,
    commands: commands::Receiver<Vec<String>>,
    notify: &mut mpsc::Sender<()>,
) -> Result<(), String> {
    if std::env::var_os("RAESIR_DISABLE_NETWORK").is_some() && url.starts_with("https://") {
        return Err("Trailer streaming is offline. Saved screenshots are still available.".into());
    }
    let api = Api::load(cache)?;
    unsafe {
        let handle = (api.create)();
        if handle.is_null() {
            return Err("The video player could not start".into());
        }
        let result = (|| {
            for (name, value) in [
                ("vo", "libmpv"),
                ("config", "no"),
                ("terminal", "no"),
                ("osd-level", "0"),
                ("keep-open", "yes"),
                ("network-timeout", "10"),
                ("sw-fast", "yes"),
                ("hwdec", "auto-safe"),
            ] {
                (api.option)(
                    handle,
                    CString::new(name).unwrap().as_ptr(),
                    CString::new(value).unwrap().as_ptr(),
                );
            }
            if std::env::var_os("ISOLATED_GUI_SESSION").is_some() {
                (api.option)(handle, c"audio".as_ptr(), c"no".as_ptr());
            }
            if (api.init)(handle) < 0 {
                return Err("The video player could not initialize".into());
            }
            let mut context = std::ptr::null_mut();
            let mut params = [
                Param {
                    kind: 1,
                    data: c"sw".as_ptr() as *mut _,
                },
                Param {
                    kind: 0,
                    data: std::ptr::null_mut(),
                },
            ];
            if (api.context)(&mut context, handle, params.as_mut_ptr()) < 0 {
                return Err("The video renderer could not initialize".into());
            }
            let (tx, rx) = commands::sync_channel(1);
            let sender = Box::new(tx);
            (api.callback)(
                context,
                Some(wake),
                (&*sender as *const SyncSender<()>) as *mut _,
            );
            let send = |args: Vec<String>| {
                let strings = args
                    .iter()
                    .map(|s| CString::new(s.as_str()).map_err(|_| "Invalid video URL"))
                    .collect::<Result<Vec<_>, _>>()?;
                let pointers = strings
                    .iter()
                    .map(|s| s.as_ptr())
                    .chain(std::iter::once(std::ptr::null()))
                    .collect::<Vec<_>>();
                if (api.command)(handle, 0, pointers.as_ptr()) < 0 {
                    return Err("The video command failed");
                }
                Ok(())
            };
            for (id, name, format) in [
                (1, c"time-pos", 5),
                (2, c"duration", 5),
                (3, c"pause", 3),
                (4, c"mute", 3),
                (5, c"volume", 5),
            ] {
                (api.observe)(handle, id, name.as_ptr(), format);
            }
            let loaded = send(vec!["loadfile".into(), url]);
            let result = (|| {
                loaded?;
                let mut size = [960i32, 540];
                let mut stride = size[0] as usize * 4;
                while !stop.load(Ordering::Relaxed) {
                    let _ = rx.recv_timeout(Duration::from_millis(40));
                    for args in commands.try_iter() {
                        send(args)?;
                    }
                    loop {
                        let event = &*(api.event)(handle, 0.);
                        if event.kind == 0 {
                            break;
                        }
                        if event.kind == 22 && !event.data.is_null() {
                            let property = &*(event.data as *const Property);
                            if !property.data.is_null() {
                                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                                if property.format == 5 {
                                    let value = *(property.data as *const f64);
                                    if value.is_finite() {
                                        match event.userdata {
                                            1 => state.position = value.max(0.),
                                            2 => state.duration = value.max(0.),
                                            5 => state.volume = value.clamp(0., 100.),
                                            _ => {}
                                        }
                                    }
                                } else if property.format == 3 {
                                    let value = *(property.data as *const c_int) != 0;
                                    match event.userdata {
                                        3 => state.paused = value,
                                        4 => state.muted = value,
                                        _ => {}
                                    }
                                }
                                drop(state);
                                let _ = notify.try_send(());
                            }
                        }
                        if event.kind == 7 && !event.data.is_null() {
                            let end = &*(event.data as *const EndFile);
                            if end.reason == 4 {
                                return Err(
                                    "This trailer could not be played. Try opening it in your player.",
                                );
                            }
                        }
                    }
                    if (api.update)(context) & 1 == 0 {
                        continue;
                    }
                    let mut pixels = vec![0u32; stride * size[1] as usize / 4];
                    let mut params = [
                        Param {
                            kind: 17,
                            data: size.as_mut_ptr().cast(),
                        },
                        Param {
                            kind: 18,
                            data: c"bgr0".as_ptr() as *mut _,
                        },
                        Param {
                            kind: 19,
                            data: (&mut stride as *mut usize).cast(),
                        },
                        Param {
                            kind: 20,
                            data: pixels.as_mut_ptr().cast(),
                        },
                        Param {
                            kind: 0,
                            data: std::ptr::null_mut(),
                        },
                    ];
                    if (api.render)(context, params.as_mut_ptr()) < 0 {
                        return Err("The video frame could not be rendered");
                    }
                    let pixels = pixels
                        .into_iter()
                        .flat_map(|pixel| {
                            let mut bytes = pixel.to_ne_bytes();
                            bytes[3] = 255;
                            bytes
                        })
                        .collect();
                    let buffer =
                        image_codec::RgbaImage::from_raw(size[0] as u32, size[1] as u32, pixels)
                            .ok_or("Invalid video frame")?;
                    state.lock().unwrap_or_else(|e| e.into_inner()).frame = Some(Arc::new(
                        RenderImage::new(vec![image_codec::Frame::new(buffer)]),
                    ));
                    let _ = notify.try_send(());
                }
                Ok(())
            })();
            (api.free)(context);
            drop(sender);
            result.map_err(str::to_owned)
        })();
        (api.terminate)(handle);
        result
    }
}
impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.cleanup_registered {
            cx.on_release_in(window, |this, window, _| {
                this.leave_fullscreen(window);
                if let Some(frame) = this.last_frame.take() {
                    let _ = window.drop_image(frame);
                }
            })
            .detach();
            self.cleanup_registered = true;
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let frame = state.frame.clone();
        let error = state.error.clone();
        let paused = state.paused;
        let muted = state.muted;
        let duration = state.duration;
        let position = self
            .drag
            .filter(|d| d.slider == Slider::Seek)
            .map(|d| d.ratio * duration)
            .unwrap_or(state.position);
        let volume = self
            .drag
            .filter(|d| d.slider == Slider::Volume)
            .map(|d| d.ratio)
            .unwrap_or(if muted { 0. } else { state.volume / 100. });
        drop(state);
        if self.last_frame.as_ref().map(|f| f.id) != frame.as_ref().map(|f| f.id) {
            if let Some(previous) = self.last_frame.take() {
                let _ = window.drop_image(previous);
            }
            self.last_frame = frame.clone();
        }
        let owner = cx.weak_entity();
        let mouse_owner = owner.clone();
        let input = canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        mouse_owner
                            .update(cx, |this, cx| this.drag_at(event.position, cx))
                            .ok();
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        owner.update(cx, |this, cx| this.finish_drag(cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        let controls = pill()
            .rounded(rems(1.2))
            .p(rems(0.65))
            .flex_col()
            .items_stretch()
            .gap(rems(0.25))
            .child(
                self.slider(
                    "video-seek",
                    Slider::Seek,
                    if duration > 0. {
                        (position / duration).clamp(0., 1.)
                    } else {
                        0.
                    },
                    cx,
                )
                .w_full(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.25))
                    .child(
                        action_button(
                            "video-pause",
                            if paused { "play" } else { "pause" },
                            if paused { "Play" } else { "Pause" },
                        )
                        .size(rems(2.25))
                        .on_click(cx.listener(|this, _, _, _| this.toggle_pause())),
                    )
                    .child(
                        action_button("video-back", "rewind", "Back 10 seconds")
                            .size(rems(2.25))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.command(&["seek", "-10", "relative"])
                            })),
                    )
                    .child(
                        action_button("video-forward", "forward", "Forward 10 seconds")
                            .size(rems(2.25))
                            .on_click(cx.listener(|this, _, _, _| {
                                this.command(&["seek", "10", "relative"])
                            })),
                    )
                    .child(div().px(rems(0.5)).text_size(rems(1.125)).child(format!(
                        "{} / {}",
                        timestamp(position),
                        timestamp(duration)
                    )))
                    .child(div().flex_1())
                    .child(
                        action_button(
                            "video-mute",
                            if muted { "muted" } else { "volume" },
                            if muted { "Unmute" } else { "Mute" },
                        )
                        .size(rems(2.25))
                        .on_click(cx.listener(|this, _, _, _| this.command(&["cycle", "mute"]))),
                    )
                    .child(
                        self.slider("video-volume", Slider::Volume, volume, cx)
                            .w(rems(5.))
                            .mx(rems(0.35)),
                    )
                    .child(
                        action_button("video-external", "external", "Open externally")
                            .size(rems(2.25))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if std::env::var_os("RAESIR_DISABLE_ACTIONS").is_none()
                                    && std::env::var_os("ISOLATED_GUI_SESSION").is_none()
                                {
                                    cx.open_url(&this.url);
                                }
                            })),
                    )
                    .child(
                        action_button(
                            "video-fullscreen",
                            if self.fullscreen {
                                "restore"
                            } else {
                                "fullscreen"
                            },
                            if self.fullscreen {
                                "Exit fullscreen"
                            } else {
                                "Fullscreen"
                            },
                        )
                        .size(rems(2.25))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_fullscreen(window, cx)),
                        ),
                    ),
            );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .child(input)
            .when_some(frame.clone(), |d, image| {
                d.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(rems(1.2))
                        .object_fit(ObjectFit::Contain),
                )
            })
            .when(frame.is_none() || error.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(rems(2.))
                        .bg(rgba(0x071410c9))
                        .rounded(rems(1.2))
                        .child(if let Some(error) = error {
                            pill()
                                .p(rems(1.))
                                .rounded(rems(1.))
                                .child(error)
                                .into_any_element()
                        } else {
                            pill()
                                .p(rems(1.))
                                .child(spinner("video-loading", 2.))
                                .into_any_element()
                        }),
                )
            })
            .child(
                div()
                    .absolute()
                    .left(rems(0.75))
                    .right(rems(0.75))
                    .bottom(rems(0.75))
                    .child(controls),
            )
    }
}
