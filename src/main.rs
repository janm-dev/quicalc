#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
	any::{self, TypeId},
	fmt::{Debug, Formatter, Result as FmtResult},
	ops::{Deref, DerefMut},
	sync::LazyLock,
	thread,
	time::Duration,
};

use global_hotkey::{
	hotkey::{Code, HotKey, Modifiers},
	GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use iced::{
	event, executor,
	futures::SinkExt,
	keyboard::{key::Named, Event as KeyboardEvent, Key, Modifiers as IcedModifiers},
	subscription,
	widget::{column, text, text_input},
	window::{
		self, icon, Event as WindowEvent, Id as WindowId, Level, Mode, Position,
		Settings as WindowSettings,
	},
	Alignment, Application, Command, Element, Event, Pixels, Settings, Size, Subscription, Theme,
};
use image::ImageFormat;
use kalk::{
	calculation_result::CalculationResult,
	parser::{eval, Context},
};
use tracing::{debug, info, trace};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static KEYBIND: LazyLock<(IcedModifiers, Key)> =
	LazyLock::new(|| (IcedModifiers::ALT, Key::Named(Named::Enter)));
static CLOSE_KEYBIND: LazyLock<(IcedModifiers, Key)> =
	LazyLock::new(|| (IcedModifiers::empty(), Key::Named(Named::Escape)));
static HOTKEY: LazyLock<HotKey> = LazyLock::new(|| HotKey::new(Some(Modifiers::ALT), Code::Enter));

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
}

#[derive(Debug, Default)]
struct Quicalc {
	ctx: ImplDebug<Context>,
	input: String,
	result: Option<ImplDebug<CalculationResult>>,
}

impl Quicalc {
	const TEXT_INPUT_ID: &'static str = "quicalc-input";
}

impl Application for Quicalc {
	type Executor = executor::Default;
	type Flags = ();
	type Message = Message;
	type Theme = Theme;

	fn new(_: Self::Flags) -> (Self, Command<Self::Message>) {
		(Self::default(), Command::none())
	}

	fn title(&self) -> String {
		"Quicalc".to_string()
	}

	fn theme(&self) -> Self::Theme {
		Self::Theme::Dark
	}

	fn subscription(&self) -> Subscription<Self::Message> {
		trace!("subscription");

		Subscription::batch([
			subscription::channel(
				TypeId::of::<GlobalHotKeyEvent>(),
				0,
				|mut sender| async move {
					loop {
						if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
							debug!(?event, "new hotkey event");

							if event.state() == HotKeyState::Pressed && event.id() == HOTKEY.id() {
								sender.send(Self::Message::ShowWindow).await.unwrap();
							}
						};

						thread::sleep(Duration::from_millis(50));
					}
				},
			),
			event::listen_with(|event, _| match event {
				Event::Keyboard(KeyboardEvent::KeyPressed { key, modifiers, .. }) => {
					let keypress = (modifiers, key);

					if keypress == *KEYBIND {
						Some(Self::Message::ShowWindow)
					} else if keypress == *CLOSE_KEYBIND {
						Some(Self::Message::HideWindow)
					} else {
						None
					}
				}
				Event::Window(WindowId::MAIN, event) => match event {
					WindowEvent::CloseRequested => Some(Self::Message::HideWindow),
					WindowEvent::Unfocused => Some(Self::Message::HideWindow),
					_ => None,
				},
				_ => None,
			}),
		])
	}

	fn update(&mut self, msg: Self::Message) -> Command<Self::Message> {
		debug!(?msg, "update");

		match msg {
			Self::Message::ShowWindow => Command::batch(vec![
				window::change_mode(WindowId::MAIN, Mode::Windowed),
				window::gain_focus(WindowId::MAIN),
				text_input::focus(text_input::Id::new(Self::TEXT_INPUT_ID)),
				text_input::select_all(text_input::Id::new(Self::TEXT_INPUT_ID)),
			]),
			Self::Message::HideWindow => {
				self.ctx.0 = Context::new();
				self.result = eval(&mut self.ctx, &self.input)
					.ok()
					.flatten()
					.map(ImplDebug);
				window::change_mode(WindowId::MAIN, Mode::Hidden)
			}
			Self::Message::InputChanged(input) => {
				self.input = input;
				self.result = eval(&mut self.ctx, &self.input)
					.ok()
					.flatten()
					.map(ImplDebug);
				Command::none()
			}
			Self::Message::InputSubmitted => Command::batch(vec![
				text_input::focus(text_input::Id::new(Self::TEXT_INPUT_ID)),
				text_input::select_all(text_input::Id::new(Self::TEXT_INPUT_ID)),
			]),
		}
	}

	fn view(&self) -> Element<'_, Self::Message, Self::Theme> {
		trace!("view");

		column![
			text_input("Do math", &self.input)
				.on_input(Self::Message::InputChanged)
				.on_submit(Self::Message::InputSubmitted)
				.id(text_input::Id::new(Self::TEXT_INPUT_ID)),
			text(
				self.result
					.as_ref()
					.map(|res| format!("= {}", res.0))
					.unwrap_or_default(),
			),
		]
		.padding(0)
		.align_items(Alignment::Start)
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
	let icon = icon::from_rgba(pixels, width, height).unwrap();

	Quicalc::run(Settings {
		antialiasing: true,
		default_text_size: Pixels(32.0),
		window: WindowSettings {
			decorations: false,
			size: Size::new(640.0, 100.0),
			position: Position::Centered,
			visible: false,
			resizable: false,
			transparent: true,
			level: Level::AlwaysOnTop,
			icon: Some(icon),
			exit_on_close_request: false,
			..Default::default()
		},
		..Default::default()
	})
	.unwrap();
}
