#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
	any,
	fmt::{Debug, Formatter, Result as FmtResult},
	ops::{Deref, DerefMut},
	sync::LazyLock,
	thread,
	time::Duration,
};

use global_hotkey::{
	GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
	hotkey::{Code, HotKey, Modifiers},
};
use iced::{
	Alignment, Element, Event, Pixels, Settings, Size, Subscription, Task, Theme, event, exit,
	futures::SinkExt,
	keyboard::{Event as KeyboardEvent, Key, Modifiers as IcedModifiers, key::Named},
	stream,
	widget::{column, text, text_input},
	window::{self, Event as WindowEvent, Level, Mode, Position, Settings as WindowSettings, icon},
};
use image::ImageFormat;
use kalk::{
	calculation_result::CalculationResult,
	parser::{Context, eval},
};
use tracing::{debug, info, trace};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use tray_icon::{
	Icon, TrayIconBuilder,
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

#[derive(Debug, Default)]
struct Quicalc {
	ctx: ImplDebug<Context>,
	input: String,
	result: Option<ImplDebug<CalculationResult>>,
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
				stream::channel(0, |mut sender| async move {
					loop {
						if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
							debug!(?event, "new hotkey event");

							if event.state() == HotKeyState::Pressed && event.id() == HOTKEY.id() {
								sender.send(Message::ShowWindow).await.unwrap();
							}
						};

						thread::sleep(Duration::from_millis(50));
					}
				})
			}),
			Subscription::run(|| {
				stream::channel(0, |mut sender| async move {
					loop {
						if let Ok(event) = MenuEvent::receiver().try_recv() {
							debug!(?event, "new tray icon menu event");

							if event.id() == &*MENU_SHOW {
								sender.send(Message::ShowWindow).await.unwrap();
							} else if event.id() == &*MENU_EXIT {
								sender.send(Message::Exit).await.unwrap();
							}
						};

						thread::sleep(Duration::from_millis(50));
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
				window::get_oldest().and_then(|id| window::change_mode(id, Mode::Windowed)),
				window::get_oldest().and_then(|id| window::gain_focus(id)),
				text_input::focus(text_input::Id::new(Self::TEXT_INPUT_ID)),
				text_input::select_all(text_input::Id::new(Self::TEXT_INPUT_ID)),
			]),
			Message::HideWindow => {
				self.ctx.0 = Context::new();
				self.result = eval(&mut self.ctx, &self.input)
					.ok()
					.flatten()
					.map(ImplDebug);
				window::get_oldest().and_then(|id| window::change_mode(id, Mode::Hidden))
			}
			Message::InputChanged(input) => {
				self.input = input;
				self.result = eval(&mut self.ctx, &self.input)
					.ok()
					.flatten()
					.map(ImplDebug);
				Task::none()
			}
			Message::InputSubmitted => Task::batch(vec![
				text_input::focus(text_input::Id::new(Self::TEXT_INPUT_ID)),
				text_input::select_all(text_input::Id::new(Self::TEXT_INPUT_ID)),
			]),
			Message::Exit => exit(),
		}
	}

	fn view(&self) -> Element<'_, Message, Theme> {
		trace!("view");

		column![
			text_input("Do math", &self.input)
				.on_input(Message::InputChanged)
				.on_submit(Message::InputSubmitted)
				.id(text_input::Id::new(Self::TEXT_INPUT_ID)),
			text(
				self.result
					.as_ref()
					.map(|res| format!("= {}", res.0))
					.unwrap_or_default(),
			),
		]
		.padding(0)
		.align_x(Alignment::Start)
		.into()
	}
}

fn main() {
	tracing_subscriber::registry()
		.with(fmt::layer())
		.with(EnvFilter::from_env("QUICALC_LOG"))
		.init();

	let hotkeys = GlobalHotKeyManager::new().unwrap();
	hotkeys.register(*HOTKEY).unwrap();

	info!("set up hotkey listener");

	let icon =
		image::load_from_memory_with_format(include_bytes!("../assets/icon.png"), ImageFormat::Png)
			.unwrap();
	let (width, height, pixels) = (icon.width(), icon.height(), icon.into_rgba8().into_vec());

	info!("loaded icon");

	let tray_menu = Menu::with_items(&[
		&MenuItem::with_id(&*MENU_SHOW.0, "Show", true, None),
		&MenuItem::with_id(&*MENU_EXIT.0, "Exit", true, None),
	])
	.unwrap();

	let _tray_icon = TrayIconBuilder::new()
		.with_tooltip("Quicalc")
		.with_icon(Icon::from_rgba(pixels.clone(), width, height).unwrap())
		.with_menu(Box::new(tray_menu))
		.build()
		.unwrap();

	info!("set up tray icon");

	iced::application(Quicalc::title, Quicalc::update, Quicalc::view)
		.subscription(Quicalc::subscription)
		.theme(Quicalc::theme)
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
			icon: Some(icon::from_rgba(pixels, width, height).unwrap()),
			exit_on_close_request: false,
			..Default::default()
		})
		.run_with(Quicalc::new)
		.unwrap();
}
