#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "python")]
use std::ffi::CString;
use std::{
	any,
	fmt::{Debug, Formatter, Result as FmtResult},
	ops::{Deref, DerefMut},
	sync::LazyLock,
};

use cfg_if::cfg_if;
use global_hotkey::{
	GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
	hotkey::{Code, HotKey, Modifiers},
};
use iced::{
	Alignment, Element, Event, Pixels, Settings, Size, Subscription, Task, Theme, event, exit,
	futures::SinkExt,
	keyboard::{Event as KeyboardEvent, Key, Modifiers as IcedModifiers, key::Named},
	stream,
	widget::{Id, Image, column, image::Handle, operation, row, text, text_input},
	window::{self, Event as WindowEvent, Level, Mode, Position, Settings as WindowSettings, icon},
};
use image::{DynamicImage, ImageFormat};
use kalk::parser::{Context, eval};
#[cfg(feature = "python")]
use pyo3::{Python, PythonVersionInfo};
#[cfg(feature = "python")]
use tracing::warn;
use tracing::{debug, error, info, trace};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use tray_icon::{
	Icon, TrayIcon, TrayIconBuilder,
	menu::{Menu, MenuEvent, MenuId, MenuItem},
};

static KEYBIND: LazyLock<(IcedModifiers, Key)> =
	LazyLock::new(|| (IcedModifiers::ALT, Key::Named(Named::Enter)));
static CLOSE_KEYBIND: LazyLock<(IcedModifiers, Key)> =
	LazyLock::new(|| (IcedModifiers::empty(), Key::Named(Named::Escape)));
static HOTKEY: LazyLock<HotKey> = LazyLock::new(|| HotKey::new(Some(Modifiers::ALT), Code::Enter));

static MENU_SHOW: LazyLock<MenuId> = LazyLock::new(|| MenuId::new("show"));
static MENU_EXIT: LazyLock<MenuId> = LazyLock::new(|| MenuId::new("exit"));

#[derive(Default, Clone, Copy)]
struct ImplDebug<T: ?Sized>(pub T);

impl<T: ?Sized> Debug for ImplDebug<T> {
	fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
		write!(f, "{}", any::type_name::<T>())
	}
}

impl<T: ?Sized> Deref for ImplDebug<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<T: ?Sized> DerefMut for ImplDebug<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

#[derive(Debug, Clone)]
enum Message {
	InputChanged(String),
	InputSubmitted,
	ShowWindow,
	HideWindow,
	Exit,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum QuicalcMode {
	#[default]
	Kalk,
	#[cfg(feature = "python")]
	Python,
}

impl QuicalcMode {
	const KALK_COMMAND: &str = "kalk";
	const PYTHON_COMMAND: &str = "py";

	fn prompt(&self) -> &'static str {
		#[cfg(feature = "python")]
		static PY_VERSION: LazyLock<String> = LazyLock::new(|| {
			Python::attach(|py| {
				let PythonVersionInfo { major, minor, .. } = py.version_info();
				format!("Python {major}.{minor}")
			})
		});

		match self {
			Self::Kalk => "Calculator",
			#[cfg(feature = "python")]
			Self::Python => &PY_VERSION,
		}
	}

	fn indicator(&self) -> &'static Handle {
		static KALK_IMAGE: LazyLock<Handle> = LazyLock::new(|| {
			let icon = image::load_from_memory_with_format(
				include_bytes!("../assets/indicators/kalk.png"),
				ImageFormat::Png,
			)
			.inspect_err(|err| error!(?err, "error loading kalk icon"))
			.unwrap_or_default();

			Handle::from_rgba(icon.width(), icon.height(), icon.into_rgba8().into_vec())
		});

		#[cfg(feature = "python")]
		static PYTHON_IMAGE: LazyLock<Handle> = LazyLock::new(|| {
			let icon = image::load_from_memory_with_format(
				include_bytes!("../assets/indicators/python.png"),
				ImageFormat::Png,
			)
			.inspect_err(|err| error!(?err, "error loading python icon"))
			.unwrap_or_default();

			Handle::from_rgba(icon.width(), icon.height(), icon.into_rgba8().into_vec())
		});

		match self {
			Self::Kalk => &KALK_IMAGE,
			#[cfg(feature = "python")]
			Self::Python => &PYTHON_IMAGE,
		}
	}
}

#[derive(Debug, Default)]
struct Quicalc {
	mode: QuicalcMode,
	ctx: ImplDebug<Context>,
	input: String,
	result: Option<String>,
}

impl Quicalc {
	const TEXT_INPUT_ID: &'static str = "quicalc-input";

	fn new() -> (Self, Task<Message>) {
		(Self::default(), Task::none())
	}

	fn title(&self) -> String {
		"Quicalc".to_string()
	}

	fn theme(&self) -> Theme {
		Theme::Dark
	}

	fn subscription(&self) -> Subscription<Message> {
		trace!("subscription");

		Subscription::batch([
			Subscription::run(|| {
				stream::channel(0, async move |mut sender| {
					loop {
						let message = crossbeam_channel::select! {
							recv(GlobalHotKeyEvent::receiver()) -> msg => {
								if let Ok(event) = msg {
									debug!(?event, "new hotkey event");

									if event.state() == HotKeyState::Pressed && event.id() == HOTKEY.id() {
										Some(Message::ShowWindow)
									} else {
										None
									}
								} else {
									error!("error receiving global hotkey event: {msg:?}");
									None
								}
							},
							recv(MenuEvent::receiver()) -> msg => {
								if let Ok(event) = msg {
									debug!(?event, "new tray icon menu event");

									if event.id() == &*MENU_SHOW {
										Some(Message::ShowWindow)
									} else if event.id() == &*MENU_EXIT {
										Some(Message::Exit)
									} else {
										error!("unknown menu item event id: {:?}", event.id());
										None
									}
								} else {
									error!("error receiving global hotkey event: {msg:?}");
									None
								}
							},
						};

						if let Some(message) = message
							&& let Err(err) = sender.send(message).await
						{
							error!("error processing event: {err:?}")
						}
					}
				})
			}),
			event::listen_with(|event, _, _| match event {
				Event::Keyboard(KeyboardEvent::KeyPressed { key, modifiers, .. }) => {
					let keypress = (modifiers, key);

					if keypress == *KEYBIND {
						Some(Message::ShowWindow)
					} else if keypress == *CLOSE_KEYBIND {
						Some(Message::HideWindow)
					} else {
						None
					}
				}
				Event::Window(event) => match event {
					WindowEvent::CloseRequested => Some(Message::HideWindow),
					WindowEvent::Unfocused => Some(Message::HideWindow),
					_ => None,
				},
				_ => None,
			}),
		])
	}

	fn update(&mut self, msg: Message) -> Task<Message> {
		debug!(?msg, "update");

		match msg {
			Message::ShowWindow => Task::batch(vec![
				window::oldest().and_then(|id| window::set_mode(id, Mode::Windowed)),
				window::oldest().and_then(window::gain_focus),
				operation::focus(Id::new(Self::TEXT_INPUT_ID)),
				operation::select_all(Id::new(Self::TEXT_INPUT_ID)),
			]),
			Message::HideWindow => {
				if self.input.is_empty() {
					self.mode = QuicalcMode::default();
				}

				self.ctx.0 = Context::new();
				self.eval();

				window::oldest().and_then(|id| window::set_mode(id, Mode::Hidden))
			}
			Message::InputChanged(input) => {
				self.input = input;
				self.eval();
				Task::none()
			}
			Message::InputSubmitted => {
				match self.input.as_str() {
					QuicalcMode::PYTHON_COMMAND => {
						cfg_if! {
							if #[cfg(feature = "python")] {
								self.mode = QuicalcMode::Python;
								self.input.clear();
								self.result = None;
							} else {
								self.input.clear();
								self.result = Some("Python mode is not supported.".to_string());
							}
						};
					}
					"" | "q" | "exit" | "quit" | "calc" | QuicalcMode::KALK_COMMAND => {
						self.mode = QuicalcMode::default();
						self.input.clear();
						self.result = None;
					}
					_ => (),
				};

				Task::batch(vec![
					operation::focus(Id::new(Self::TEXT_INPUT_ID)),
					operation::select_all(Id::new(Self::TEXT_INPUT_ID)),
				])
			}
			Message::Exit => exit(),
		}
	}

	fn view(&self) -> Element<'_, Message, Theme> {
		trace!("view");

		column![
			text_input(self.mode.prompt(), &self.input)
				.on_input(Message::InputChanged)
				.on_submit(Message::InputSubmitted)
				.id(Id::new(Self::TEXT_INPUT_ID)),
			row![
				Image::new(self.mode.indicator()),
				text(self.result.as_deref().unwrap_or_default())
			],
		]
		.padding(0)
		.align_x(Alignment::Start)
		.into()
	}

	fn eval(&mut self) {
		trace!("eval");

		match self.mode {
			QuicalcMode::Kalk => {
				self.result = eval(&mut self.ctx, &self.input)
					.inspect_err(|err| debug!(?err, "error evaluating math"))
					.ok()
					.flatten()
					.map(|res| format!("≈ {res}"));
			}
			#[cfg(feature = "python")]
			QuicalcMode::Python => {
				self.result = Python::attach(|py| {
					py.eval(
						CString::new(self.input.clone())
							.inspect_err(|err| warn!(?err, "invalid python expression entered"))
							.unwrap_or_default()
							.as_c_str(),
						None,
						None,
					)
					.inspect_err(|err| debug!(?err, "error evaluating python expression"))
					.ok()
					.map(|res| format!("→ {res}"))
				})
			}
		}
	}
}

fn set_up_hotkey() -> Result<GlobalHotKeyManager, String> {
	let hotkeys = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
	hotkeys.register(*HOTKEY).map_err(|e| e.to_string())?;

	Ok(hotkeys)
}

fn set_up_tray_icon(icon: &DynamicImage) -> Result<TrayIcon, String> {
	let tray_menu = Menu::with_items(&[
		&MenuItem::with_id(&*MENU_SHOW.0, "Show", true, None),
		&MenuItem::with_id(&*MENU_EXIT.0, "Exit", true, None),
	])
	.map_err(|e| e.to_string())?;

	let (width, height, pixels) = (icon.width(), icon.height(), icon.to_rgba8().into_vec());

	let tray_icon = TrayIconBuilder::new()
		.with_tooltip("Quicalc")
		.with_icon(Icon::from_rgba(pixels, width, height).map_err(|e| e.to_string())?)
		.with_menu(Box::new(tray_menu))
		.build()
		.map_err(|e| e.to_string())?;

	Ok(tray_icon)
}

fn main() {
	tracing_subscriber::registry()
		.with(fmt::layer())
		.with(EnvFilter::from_env("QUICALC_LOG"))
		.init();

	let _hotkeys = set_up_hotkey()
		.inspect(|_| info!("set up global hotkey"))
		.inspect_err(|err| error!(?err, "error setting up global hotkey"))
		.ok();

	let icon =
		image::load_from_memory_with_format(include_bytes!("../assets/icon.png"), ImageFormat::Png)
			.inspect_err(|err| error!(?err, "error loading program icon"))
			.unwrap_or_default();

	info!("loaded icon");

	let _tray_icon = set_up_tray_icon(&icon)
		.inspect(|_| info!("set up tray icon"))
		.inspect_err(|err| error!(?err, "error setting up tray icon"))
		.ok();

	let (width, height, pixels) = (icon.width(), icon.height(), icon.into_rgba8().into_vec());
	let window_icon = icon::from_rgba(pixels, width, height)
		.inspect_err(|err| error!(?err, "error setting window icon"))
		.ok();

	iced::application(Quicalc::new, Quicalc::update, Quicalc::view)
		.subscription(Quicalc::subscription)
		.theme(Quicalc::theme)
		.title(Quicalc::title)
		.settings(Settings {
			antialiasing: true,
			default_text_size: Pixels(32.0),
			..Default::default()
		})
		.window(WindowSettings {
			decorations: false,
			size: Size::new(640.0, 100.0),
			position: Position::Centered,
			visible: false,
			resizable: false,
			transparent: true,
			level: Level::AlwaysOnTop,
			icon: window_icon,
			exit_on_close_request: false,
			..Default::default()
		})
		.run()
		.inspect_err(|err| error!(?err, "error running application"))
		.unwrap();
}
