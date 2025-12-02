use std::{
  cell::RefCell,
  collections::HashMap,
  panic,
  sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc, Mutex,
  },
};

use raw_window_handle::{HasWindowHandle, WindowHandle};
use tauri_runtime::{
  dpi::{self, PhysicalPosition, PhysicalSize, Position, Size},
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{
    CursorIcon, DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowBuilder,
    WindowBuilderBase, WindowEvent, WindowId,
  },
  ExitRequestedEventAction, Icon, ProgressBarState, Result, RunEvent, UserAttentionType, UserEvent,
  WindowEventId,
};

#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;
use tauri_utils::{
  config::{Color, WindowConfig},
  Theme,
};

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use crate::{
  getter,
  webview::{CefWebviewDispatcher, Webview, WebviewKind},
  window_getter, CefRuntime, CefRuntimeContext, Message,
};

pub type WindowEventHandler = Box<dyn Fn(&WindowEvent) + Send>;
pub type WindowEventListeners = Arc<Mutex<HashMap<WindowEventId, WindowEventHandler>>>;
pub type WebviewEventHandler = Box<dyn Fn(&tauri_runtime::window::WebviewEvent) + Send>;
pub type WebviewEventListeners =
  Arc<Mutex<HashMap<u32, Arc<Mutex<HashMap<tauri_runtime::WebviewEventId, WebviewEventHandler>>>>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
  /// Full browser window created with browser_host_create_browser_sync
  Browser,
  /// Tauri window created with window_create_top_level
  Tauri,
}

pub(crate) enum AppWindowKind {
  Window(Arc<Box<dyn winit::window::Window>>),
  BrowserWindow,
}

pub struct Window {
  pub label: String,
  pub window: AppWindowKind,
  pub force_close: Arc<AtomicBool>,
  pub attributes: Arc<RefCell<CefWindowBuilder>>,
  pub webviews: Vec<Webview>,
  pub window_event_listeners: WindowEventListeners,
  pub webview_event_listeners: WebviewEventListeners,
}

impl std::fmt::Debug for Window {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Window")
      .field("label", &self.label)
      .field("window", &"AppWindowKind")
      .field("force_close", &self.force_close)
      .field("attributes", &self.attributes)
      .field("webviews", &self.webviews)
      .finish()
  }
}

impl Window {
  pub fn window(&self) -> Option<Arc<Box<dyn winit::window::Window>>> {
    match &self.window {
      AppWindowKind::Window(window) => Some(window.clone()),
      AppWindowKind::BrowserWindow => None,
    }
  }
}

pub enum WindowMessage {
  Close,
  Destroy,
  AddEventListener(WindowEventId, Box<dyn Fn(&WindowEvent) + Send>),
  // Getters
  ScaleFactor(Sender<Result<f64>>),
  InnerPosition(Sender<Result<PhysicalPosition<i32>>>),
  OuterPosition(Sender<Result<PhysicalPosition<i32>>>),
  InnerSize(Sender<Result<PhysicalSize<u32>>>),
  OuterSize(Sender<Result<PhysicalSize<u32>>>),
  IsFullscreen(Sender<Result<bool>>),
  IsMinimized(Sender<Result<bool>>),
  IsMaximized(Sender<Result<bool>>),
  IsFocused(Sender<Result<bool>>),
  IsDecorated(Sender<Result<bool>>),
  IsResizable(Sender<Result<bool>>),
  IsMaximizable(Sender<Result<bool>>),
  IsMinimizable(Sender<Result<bool>>),
  IsClosable(Sender<Result<bool>>),
  IsVisible(Sender<Result<bool>>),
  Title(Sender<Result<String>>),
  CurrentMonitor(Sender<Result<Option<Monitor>>>),
  PrimaryMonitor(Sender<Result<Option<Monitor>>>),
  MonitorFromPoint(Sender<Result<Option<Monitor>>>, f64, f64),
  AvailableMonitors(Sender<Result<Vec<Monitor>>>),
  Theme(Sender<Result<Theme>>),
  IsEnabled(Sender<Result<bool>>),
  IsAlwaysOnTop(Sender<Result<bool>>),
  RawWindowHandle(
    Sender<
      std::result::Result<raw_window_handle::WindowHandle<'static>, raw_window_handle::HandleError>,
    >,
  ),
  // Setters
  Center,
  RequestUserAttention(Option<UserAttentionType>),
  SetEnabled(bool),
  SetResizable(bool),
  SetMaximizable(bool),
  SetMinimizable(bool),
  SetClosable(bool),
  SetTitle(String),
  Maximize,
  Unmaximize,
  Minimize,
  Unminimize,
  Show,
  Hide,
  SetDecorations(bool),
  SetShadow(bool),
  SetAlwaysOnBottom(bool),
  SetAlwaysOnTop(bool),
  SetVisibleOnAllWorkspaces(bool),
  SetContentProtected(bool),
  SetSize(Size),
  SetMinSize(Option<Size>),
  SetMaxSize(Option<Size>),
  SetSizeConstraints(tauri_runtime::window::WindowSizeConstraints),
  SetPosition(Position),
  SetFullscreen(bool),
  #[cfg(target_os = "macos")]
  SetSimpleFullscreen(bool),
  SetFocus,
  SetFocusable(bool),
  SetIcon(Icon<'static>),
  SetSkipTaskbar(bool),
  SetCursorGrab(bool),
  SetCursorVisible(bool),
  SetCursorIcon(CursorIcon),
  SetCursorPosition(Position),
  SetIgnoreCursorEvents(bool),
  SetProgressBar(ProgressBarState),
  SetBadgeCount(Option<i64>, Option<String>),
  SetBadgeLabel(Option<String>),
  SetOverlayIcon(Option<Icon<'static>>),
  SetTitleBarStyle(tauri_utils::TitleBarStyle),
  SetTrafficLightPosition(Position),
  SetTheme(Option<Theme>),
  SetBackgroundColor(Option<Color>),
  StartDragging,
  StartResizeDragging(tauri_runtime::ResizeDirection),
}

#[derive(Debug, Clone, Default)]
pub struct CefWindowBuilder {
  pub attrs: winit::window::WindowAttributes,
  pub inner_size_constraints: Option<tauri_runtime::window::WindowSizeConstraints>,
  #[cfg(target_os = "macos")]
  pub macos_attrs: winit::platform::macos::WindowAttributesMacOS,
  pub browser_window: bool,
  pub center: bool,
}

impl CefWindowBuilder {
  pub fn browser_window(mut self) -> Self {
    self.browser_window = true;
    self
  }
}

impl WindowBuilderBase for CefWindowBuilder {}
impl WindowBuilder for CefWindowBuilder {
  fn new() -> Self {
    Self::default()
  }

  fn with_config(config: &WindowConfig) -> Self {
    let mut builder = Self::new();

    builder = builder
      .title(config.title.to_string())
      .inner_size(config.width, config.height)
      .focused(config.focus)
      .focusable(config.focusable)
      .visible(config.visible)
      .resizable(config.resizable)
      .fullscreen(config.fullscreen)
      .decorations(config.decorations)
      .maximized(config.maximized)
      .always_on_bottom(config.always_on_bottom)
      .always_on_top(config.always_on_top)
      .visible_on_all_workspaces(config.visible_on_all_workspaces)
      .content_protected(config.content_protected)
      .skip_taskbar(config.skip_taskbar)
      .theme(config.theme)
      .closable(config.closable)
      .maximizable(config.maximizable)
      .minimizable(config.minimizable)
      .shadow(config.shadow);

    let mut constraints = tauri_runtime::window::WindowSizeConstraints::default();
    if let Some(min_width) = config.min_width {
      constraints.min_width = Some(tauri_runtime::dpi::LogicalUnit::new(min_width).into());
    }
    if let Some(min_height) = config.min_height {
      constraints.min_height = Some(tauri_runtime::dpi::LogicalUnit::new(min_height).into());
    }
    if let Some(max_width) = config.max_width {
      constraints.max_width = Some(tauri_runtime::dpi::LogicalUnit::new(max_width).into());
    }
    if let Some(max_height) = config.max_height {
      constraints.max_height = Some(tauri_runtime::dpi::LogicalUnit::new(max_height).into());
    }
    builder = builder.inner_size_constraints(constraints);

    if let Some(color) = config.background_color {
      builder = builder.background_color(color);
    }

    if let (Some(x), Some(y)) = (config.x, config.y) {
      builder = builder.position(x, y);
    }

    if config.center {
      builder = builder.center();
    }

    #[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]
    {
      builder = builder.transparent(config.transparent);
    }

    #[cfg(target_os = "macos")]
    {
      builder = builder
        .hidden_title(config.hidden_title)
        .title_bar_style(config.title_bar_style);
      if let Some(identifier) = &config.tabbing_identifier {
        builder = builder.tabbing_identifier(identifier);
      }
      if let Some(position) = &config.traffic_light_position {
        builder = builder.traffic_light_position(tauri_runtime::dpi::LogicalPosition::new(
          position.x, position.y,
        ));
      }
    }

    #[cfg(windows)]
    {
      if let Some(window_classname) = &config.window_classname {
        builder = builder.window_classname(window_classname);
      }
    }

    builder
  }

  fn center(mut self) -> Self {
    self.center = true;
    self
  }

  fn position(mut self, x: f64, y: f64) -> Self {
    let position = winit::dpi::LogicalPosition::new(x, y);
    self.attrs = self.attrs.with_position(position);
    self
  }

  fn inner_size(mut self, width: f64, height: f64) -> Self {
    let size = winit::dpi::LogicalSize::new(width, height);
    self.attrs = self.attrs.with_surface_size(size);
    self
  }

  fn min_inner_size(mut self, min_width: f64, min_height: f64) -> Self {
    let size = winit::dpi::LogicalSize::new(min_width, min_height);
    self.attrs = self.attrs.with_min_surface_size(size);
    self
  }

  fn max_inner_size(mut self, max_width: f64, max_height: f64) -> Self {
    let size = winit::dpi::LogicalSize::new(max_width, max_height);
    self.attrs = self.attrs.with_max_surface_size(size);
    self
  }

  fn inner_size_constraints(
    mut self,
    constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Self {
    self.inner_size_constraints = Some(constraints);
    self
  }

  fn prevent_overflow(mut self) -> Self {
    // TODO
    self
  }

  fn prevent_overflow_with_margin(mut self, margin: dpi::Size) -> Self {
    // TODO
    self
  }

  fn resizable(mut self, resizable: bool) -> Self {
    self.attrs = self.attrs.with_resizable(resizable);
    self
  }

  fn maximizable(mut self, maximizable: bool) -> Self {
    use winit::window::WindowButtons;

    let mut buttons = self.attrs.enabled_buttons;
    buttons.set(WindowButtons::MAXIMIZE, maximizable);

    self.attrs = self.attrs.with_enabled_buttons(buttons);
    self
  }

  fn minimizable(mut self, minimizable: bool) -> Self {
    use winit::window::WindowButtons;

    let mut buttons = self.attrs.enabled_buttons;
    buttons.set(WindowButtons::MINIMIZE, minimizable);

    self.attrs = self.attrs.with_enabled_buttons(buttons);
    self
  }

  fn closable(mut self, closable: bool) -> Self {
    use winit::window::WindowButtons;

    let mut buttons = self.attrs.enabled_buttons;
    buttons.set(WindowButtons::CLOSE, closable);

    self.attrs = self.attrs.with_enabled_buttons(buttons);
    self
  }

  fn title<S: Into<String>>(mut self, title: S) -> Self {
    self.attrs = self.attrs.with_title(title.into());
    self
  }

  fn fullscreen(mut self, fullscreen: bool) -> Self {
    use winit::monitor::Fullscreen;

    let fullscreen = if fullscreen {
      Some(Fullscreen::Borderless(None))
    } else {
      None
    };

    self.attrs = self.attrs.with_fullscreen(fullscreen);
    self
  }

  fn focused(mut self, focused: bool) -> Self {
    self.attrs = self.attrs.with_active(focused);
    self
  }

  fn focusable(mut self, focusable: bool) -> Self {
    self.attrs = self.attrs.with_active(focusable);
    self
  }

  fn maximized(mut self, maximized: bool) -> Self {
    self.attrs = self.attrs.with_maximized(maximized);
    self
  }

  fn visible(mut self, visible: bool) -> Self {
    self.attrs = self.attrs.with_visible(visible);
    self
  }

  #[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]
  fn transparent(mut self, transparent: bool) -> Self {
    self.attrs = self.attrs.with_transparent(transparent);
    self
  }

  fn decorations(mut self, decorations: bool) -> Self {
    self.attrs = self.attrs.with_decorations(decorations);
    self
  }

  fn always_on_bottom(mut self, always_on_bottom: bool) -> Self {
    use winit::window::WindowLevel;
    let level = if always_on_bottom {
      WindowLevel::AlwaysOnBottom
    } else {
      WindowLevel::Normal
    };

    self.attrs = self.attrs.with_window_level(level);
    self
  }

  fn always_on_top(mut self, always_on_top: bool) -> Self {
    use winit::window::WindowLevel;
    let level = if always_on_top {
      WindowLevel::AlwaysOnTop
    } else {
      WindowLevel::Normal
    };

    self.attrs = self.attrs.with_window_level(level);
    self
  }

  fn visible_on_all_workspaces(mut self, visible_on_all_workspaces: bool) -> Self {
    self
  }

  fn content_protected(mut self, protected: bool) -> Self {
    self.attrs = self.attrs.with_content_protected(protected);
    self
  }

  fn icon(mut self, icon: Icon) -> tauri_runtime::Result<Self> {
    use winit::icon::RgbaIcon;

    let icon = RgbaIcon::new(icon.rgba.into(), icon.width, icon.height)
      .map_err(|e| tauri_runtime::Error::InvalidIcon(e.into()))?;

    self.attrs = self.attrs.with_window_icon(Some(icon.into()));

    Ok(self)
  }

  #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
  fn skip_taskbar(self, _skip: bool) -> Self {
    self
  }

  fn background_color(mut self, color: Color) -> Self {
    // TODO
    self
  }

  fn shadow(mut self, enable: bool) -> Self {
    #[cfg(windows)]
    {
      self.attrs = self.attrs.with_undecorated_shadow(enable);
    }

    #[cfg(target_os = "macos")]
    {
      self.macos_attrs = self.macos_attrs.with_has_shadow(enable);
      let macos_attrs = Box::new(self.macos_attrs.clone());
      self.attrs = self.attrs.with_platform_attributes(macos_attrs);
    }

    self
  }

  #[cfg(target_os = "macos")]
  fn parent(mut self, parent: *mut std::ffi::c_void) -> Self {
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};

    let parent = std::ptr::NonNull::new(parent).expect("msg");
    let parent = AppKitWindowHandle::new(parent);
    let parent = RawWindowHandle::AppKit(parent);

    self.attrs = unsafe { self.attrs.with_parent_window(Some(parent)) };
    self
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(mut self, style: TitleBarStyle) -> Self {
    match style {
      TitleBarStyle::Visible => {
        self.macos_attrs = self.macos_attrs.with_titlebar_transparent(false);
        // Fixes rendering issue when resizing window with devtools open (https://github.com/tauri-apps/tauri/issues/3914)
        self.macos_attrs = self.macos_attrs.with_fullsize_content_view(true);
      }
      TitleBarStyle::Transparent => {
        self.macos_attrs = self.macos_attrs.with_titlebar_transparent(true);
        self.macos_attrs = self.macos_attrs.with_fullsize_content_view(false);
      }
      TitleBarStyle::Overlay => {
        self.macos_attrs = self.macos_attrs.with_titlebar_transparent(true);
        self.macos_attrs = self.macos_attrs.with_fullsize_content_view(true);
      }
      unknown => {
        #[cfg(feature = "tracing")]
        tracing::warn!("unknown title bar style applied: {unknown}");

        #[cfg(not(feature = "tracing"))]
        eprintln!("unknown title bar style applied: {unknown}");
      }
    }

    let macos_attrs = Box::new(self.macos_attrs.clone());
    self.attrs = self.attrs.with_platform_attributes(macos_attrs);

    self
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(mut self, position: P) -> Self {
    // TODO
    self
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(mut self, hidden: bool) -> Self {
    self.macos_attrs = self.macos_attrs.with_title_hidden(hidden);
    let macos_attrs = Box::new(self.macos_attrs.clone());
    self.attrs = self.attrs.with_platform_attributes(macos_attrs);
    self
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(mut self, identifier: &str) -> Self {
    self.macos_attrs = self.macos_attrs.with_tabbing_identifier(identifier.into());
    let macos_attrs = Box::new(self.macos_attrs.clone());
    self.attrs = self.attrs.with_platform_attributes(macos_attrs);
    self
  }

  fn theme(mut self, theme: Option<Theme>) -> Self {
    self.attrs = self.attrs.with_theme(match theme {
      Some(Theme::Light) => Some(winit::window::Theme::Light),
      Some(Theme::Dark) => Some(winit::window::Theme::Dark),
      _ => None,
    });
    self
  }

  #[cfg(not(windows))]
  fn window_classname<S: Into<String>>(self, _window_classname: S) -> Self {
    self
  }

  fn has_icon(&self) -> bool {
    self.attrs.window_icon.is_some()
  }

  fn get_theme(&self) -> Option<Theme> {
    match self.attrs.preferred_theme {
      Some(winit::window::Theme::Light) => Some(Theme::Light),
      Some(winit::window::Theme::Dark) => Some(Theme::Dark),
      _ => None,
    }
  }
}

#[derive(Debug, Clone)]
pub struct CefWindowDispatcher<T: UserEvent> {
  window_id: WindowId,
  context: CefRuntimeContext<T>,
}

impl<T: UserEvent> tauri_runtime::WindowDispatch<T> for CefWindowDispatcher<T> {
  type Runtime = CefRuntime<T>;

  type WindowBuilder = CefWindowBuilder;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.proxy.send_message(Message::Task(Box::new(f)))
  }

  fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, f: F) -> WindowEventId {
    let context = self.context.clone();
    let window_id = self.window_id;
    let event_id = context.next_window_event_id();
    let handler = Box::new(f);

    // Register the listener on the main thread
    let _ = context.proxy.send_message(Message::Window {
      window_id,
      message: WindowMessage::AddEventListener(event_id, handler),
    });

    event_id
  }

  fn scale_factor(&self) -> Result<f64> {
    window_getter!(self, WindowMessage::ScaleFactor)?
  }

  fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
    window_getter!(self, WindowMessage::InnerPosition)?
  }

  fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
    window_getter!(self, WindowMessage::OuterPosition)?
  }

  fn inner_size(&self) -> Result<PhysicalSize<u32>> {
    window_getter!(self, WindowMessage::InnerSize)?
  }

  fn outer_size(&self) -> Result<PhysicalSize<u32>> {
    window_getter!(self, WindowMessage::OuterSize)?
  }

  fn is_fullscreen(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsFullscreen)?
  }

  fn is_minimized(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsMinimized)?
  }

  fn is_maximized(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsMaximized)?
  }

  fn is_focused(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsFocused)?
  }

  fn is_decorated(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsDecorated)?
  }

  fn is_resizable(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsResizable)?
  }

  fn is_maximizable(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsMaximizable)?
  }

  fn is_minimizable(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsMinimizable)?
  }

  fn is_closable(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsClosable)?
  }

  fn is_visible(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsVisible)?
  }

  fn title(&self) -> Result<String> {
    window_getter!(self, WindowMessage::Title)?
  }

  fn current_monitor(&self) -> Result<Option<Monitor>> {
    window_getter!(self, WindowMessage::CurrentMonitor)?
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    window_getter!(self, WindowMessage::PrimaryMonitor)?
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    let (tx, rx) = std::sync::mpsc::channel();
    getter!(
      self,
      rx,
      Message::Window {
        window_id: self.window_id,
        message: WindowMessage::MonitorFromPoint(tx, x, y)
      }
    )?
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    window_getter!(self, WindowMessage::AvailableMonitors)?
  }

  fn theme(&self) -> Result<Theme> {
    window_getter!(self, WindowMessage::Theme)?
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn gtk_window(&self) -> Result<gtk::ApplicationWindow> {
    unimplemented!()
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn default_vbox(&self) -> Result<gtk::Box> {
    unimplemented!()
  }

  fn window_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
    let (tx, rx) = std::sync::mpsc::channel();
    self
      .context
      .proxy
      .send_message(Message::Window {
        window_id: self.window_id,
        message: WindowMessage::RawWindowHandle(tx),
      })
      .map_err(|_| raw_window_handle::HandleError::Unavailable)?;
    rx.recv()
      .map_err(|_| raw_window_handle::HandleError::Unavailable)?
  }

  fn center(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Center,
    })
  }

  fn request_user_attention(&self, request_type: Option<UserAttentionType>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::RequestUserAttention(request_type),
    })
  }

  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &mut self,
    pending: PendingWindow<T, Self::Runtime>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    self.context.create_window(pending, after_window_creation)
  }

  fn create_webview(
    &mut self,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    self.context.create_webview(self.window_id, pending)
  }

  fn set_resizable(&self, resizable: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetResizable(resizable),
    })
  }

  fn set_maximizable(&self, maximizable: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetMaximizable(maximizable),
    })
  }

  fn set_minimizable(&self, minimizable: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetMinimizable(minimizable),
    })
  }

  fn set_closable(&self, closable: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetClosable(closable),
    })
  }

  fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetTitle(title.into()),
    })
  }

  fn maximize(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Maximize,
    })
  }

  fn unmaximize(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Unmaximize,
    })
  }

  fn minimize(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Minimize,
    })
  }

  fn unminimize(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Unminimize,
    })
  }

  fn show(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Show,
    })
  }

  fn hide(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Hide,
    })
  }

  fn close(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Close,
    })
  }

  fn destroy(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::Destroy,
    })
  }

  fn set_decorations(&self, decorations: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetDecorations(decorations),
    })
  }

  fn set_shadow(&self, shadow: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetShadow(shadow),
    })
  }

  fn set_always_on_bottom(&self, always_on_bottom: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetAlwaysOnBottom(always_on_bottom),
    })
  }

  fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetAlwaysOnTop(always_on_top),
    })
  }

  fn set_visible_on_all_workspaces(&self, visible_on_all_workspaces: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetVisibleOnAllWorkspaces(visible_on_all_workspaces),
    })
  }

  fn set_content_protected(&self, protected: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetContentProtected(protected),
    })
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetSize(size),
    })
  }

  fn set_min_size(&self, size: Option<Size>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetMinSize(size),
    })
  }

  fn set_max_size(&self, size: Option<Size>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetMaxSize(size),
    })
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetPosition(position),
    })
  }

  fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetFullscreen(fullscreen),
    })
  }

  #[cfg(target_os = "macos")]
  fn set_simple_fullscreen(&self, fullscreen: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetSimpleFullscreen(fullscreen),
    })
  }

  fn set_focus(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetFocus,
    })
  }

  fn set_focusable(&self, focusable: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetFocusable(focusable),
    })
  }

  fn set_icon(&self, icon: Icon<'_>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetIcon(icon.into_owned()),
    })
  }

  fn set_skip_taskbar(&self, skip: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetSkipTaskbar(skip),
    })
  }

  fn set_cursor_grab(&self, grab: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetCursorGrab(grab),
    })
  }

  fn set_cursor_visible(&self, visible: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetCursorVisible(visible),
    })
  }

  fn set_cursor_icon(&self, icon: CursorIcon) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetCursorIcon(icon),
    })
  }

  fn set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetCursorPosition(position.into()),
    })
  }

  fn set_ignore_cursor_events(&self, ignore: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetIgnoreCursorEvents(ignore),
    })
  }

  fn start_dragging(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::StartDragging,
    })
  }

  fn start_resize_dragging(&self, direction: tauri_runtime::ResizeDirection) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::StartResizeDragging(direction),
    })
  }

  fn set_progress_bar(&self, progress_state: ProgressBarState) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetProgressBar(progress_state),
    })
  }

  fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetBadgeCount(count, desktop_filename),
    })
  }

  fn set_badge_label(&self, label: Option<String>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetBadgeLabel(label),
    })
  }

  fn set_overlay_icon(&self, icon: Option<Icon<'_>>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetOverlayIcon(icon.map(|i| i.into_owned())),
    })
  }

  fn set_title_bar_style(&self, style: tauri_utils::TitleBarStyle) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetTitleBarStyle(style),
    })
  }

  fn set_traffic_light_position(&self, position: Position) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetTrafficLightPosition(position),
    })
  }

  fn set_size_constraints(
    &self,
    constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetSizeConstraints(constraints),
    })
  }

  fn set_theme(&self, theme: Option<Theme>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetTheme(theme),
    })
  }

  fn set_enabled(&self, enabled: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetEnabled(enabled),
    })
  }

  fn is_enabled(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsEnabled)?
  }

  fn is_always_on_top(&self) -> Result<bool> {
    window_getter!(self, WindowMessage::IsAlwaysOnTop)?
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    self.context.proxy.send_message(Message::Window {
      window_id: self.window_id,
      message: WindowMessage::SetBackgroundColor(color),
    })
  }
}

impl<T: UserEvent> CefRuntimeContext<T> {
  pub fn create_window<F: Fn(RawWindow) + Send + 'static>(
    &self,
    pending: PendingWindow<T, CefRuntime<T>>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, CefRuntime<T>>> {
    let label = pending.label.clone();
    let context = self.clone();
    let window_id = self.next_window_id();
    let (webview_id, use_https_scheme) = pending
      .webview
      .as_ref()
      .map(|w| {
        (
          Some(context.next_webview_id()),
          w.webview_attributes.use_https_scheme,
        )
      })
      .unwrap_or((None, false));

    self.proxy.send_message(Message::CreateWindow {
      window_id,
      webview_id: webview_id.unwrap_or_default(),
      pending,
      after_window_creation: after_window_creation
        .map(|f| Box::new(f) as Box<dyn Fn(RawWindow) + Send + 'static>),
    })?;

    let dispatcher = CefWindowDispatcher {
      window_id,
      context: self.clone(),
    };

    let detached_webview = webview_id.map(|id| {
      let webview = DetachedWebview {
        label: label.clone(),
        dispatcher: CefWebviewDispatcher {
          window_id: Arc::new(Mutex::new(window_id)),
          webview_id: id,
          context: self.clone(),
        },
      };
      DetachedWindowWebview {
        webview,
        use_https_scheme,
      }
    });

    Ok(DetachedWindow {
      id: window_id,
      label,
      dispatcher,
      webview: detached_webview,
    })
  }
}

pub(crate) fn create_window<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  event_loop: &dyn winit::event_loop::ActiveEventLoop,
  window_id: WindowId,
  webview_id: u32,
  pending: PendingWindow<T, CefRuntime<T>>,
) {
  let PendingWindow {
    label,
    window_builder,
    webview,
  } = pending;

  if window_builder.browser_window {
    panic!("browser_window is not imlemented yet for CefRuntime");
  }

  let force_close = Arc::new(AtomicBool::new(false));
  let attributes = Arc::new(RefCell::new(window_builder));

  let window = event_loop
    .create_window(attributes.borrow().attrs.clone())
    .expect("failed to create window");

  context.windows.borrow_mut().insert(
    window_id,
    crate::window::Window {
      label,
      window: AppWindowKind::Window(Arc::new(window)),
      force_close,
      attributes,
      webviews: Vec::new(),
      window_event_listeners: Arc::new(Mutex::new(HashMap::new())),
      webview_event_listeners: Arc::new(Mutex::new(HashMap::new())),
    },
  );

  if let Some(webview) = webview {
    crate::webview::create_webview(
      context,
      WebviewKind::WindowContent,
      window_id,
      webview_id,
      webview,
    );
  }
}

fn send_window_event<T: UserEvent>(
  window_id: WindowId,
  windows: &Arc<RefCell<HashMap<WindowId, crate::window::Window>>>,
  callback: &Arc<RefCell<Box<dyn Fn(RunEvent<T>)>>>,
  event: WindowEvent,
) {
  let Ok(windows_ref) = windows.try_borrow() else {
    // TODO:
    // // post task to run later - windows currently mutably borrowed
    // // happens usually on reparent or destroy when there's a focus change event
    // let mut task =
    //   WindowEventTask::new(window_id, windows.clone(), callback.clone(), event.clone());

    // cef::post_task(sys::cef_thread_id_t::TID_UI.into(), Some(&mut task));
    return;
  };

  if let Some(w) = windows_ref.get(&window_id) {
    let label = w.label.clone();
    let window_event_listeners = w.window_event_listeners.clone();

    drop(windows_ref);

    {
      let listeners = window_event_listeners.lock().unwrap();
      let handlers: Vec<_> = listeners.values().collect();
      for handler in handlers.iter() {
        handler(&event);
      }
    }

    (callback.borrow())(RunEvent::WindowEvent { label, event });
  }
}

fn on_close_requested<T: UserEvent>(
  window_id: WindowId,
  windows: &Arc<RefCell<HashMap<WindowId, crate::window::Window>>>,
  callback: &Arc<RefCell<Box<dyn Fn(RunEvent<T>)>>>,
) {
  let (tx, rx) = std::sync::mpsc::channel();
  let event = WindowEvent::CloseRequested { signal_tx: tx };

  send_window_event(window_id, windows, callback, event.clone());

  let prevent = rx.try_recv().unwrap_or_default();

  if !prevent {
    on_window_close(window_id, windows);
  }
}

fn on_window_close(
  window_id: WindowId,
  windows: &Arc<RefCell<HashMap<WindowId, crate::window::Window>>>,
) {
  if let Some(app_window) = windows.borrow().get(&window_id) {
    app_window.force_close.store(true, Ordering::SeqCst);
    if let Some(window) = app_window.window() {
      // TODO:
      // window.close();
    }
    // For BrowserWindow, we can't close it directly, but the browser will handle it
  }
}

fn on_window_destroyed<T: UserEvent>(
  window_id: WindowId,
  windows: &Arc<RefCell<HashMap<WindowId, crate::window::Window>>>,
  callback: &Arc<RefCell<Box<dyn Fn(RunEvent<T>)>>>,
) {
  let event = WindowEvent::Destroyed;
  send_window_event(window_id, windows, callback, event);

  let removed = windows.borrow_mut().remove(&window_id).is_some();

  if removed {
    let is_empty = windows.borrow().is_empty();
    if is_empty {
      let (tx, rx) = std::sync::mpsc::channel();
      (callback.borrow())(RunEvent::ExitRequested { code: None, tx });

      let recv = rx.try_recv();
      let should_prevent = matches!(recv, Ok(ExitRequestedEventAction::Prevent));

      if !should_prevent {
        (callback.borrow())(RunEvent::Exit);
      }
    }
  }
}

pub fn handle_window_message<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  event_loop: &dyn winit::event_loop::ActiveEventLoop,
  window_id: WindowId,
  message: WindowMessage,
) {
  match message {
    WindowMessage::Close => {
      // TODO:
      // on_close_requested(window_id, &context.windows, &context.callback);
    }
    WindowMessage::Destroy => {
      on_window_close(window_id, &context.windows);
    }
    WindowMessage::AddEventListener(event_id, handler) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        app_window
          .window_event_listeners
          .lock()
          .unwrap()
          .insert(event_id, handler);
      }
    }
    // Getters
    WindowMessage::ScaleFactor(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|w| w.window().map(|window| window.scale_factor()))
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::InnerPosition(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => window
            .outer_position()
            .map_err(|_| tauri_runtime::Error::FailedToSendMessage),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::OuterPosition(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => window
            .outer_position()
            .map_err(|_| tauri_runtime::Error::FailedToSendMessage),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::InnerSize(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.surface_size()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::OuterSize(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.outer_size()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsFullscreen(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.fullscreen().is_some()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsMinimized(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => {
            Ok(window.is_minimized().unwrap_or_default())
          }
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsMaximized(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.is_maximized()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsFocused(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.has_focus()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsDecorated(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          if let Some(window) = w.window() {
            window.is_decorated()
          } else {
            false
          }
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsResizable(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          if let Some(window) = w.window() {
            window.is_resizable()
          } else {
            false
          }
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsMaximizable(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          if let Some(window) = w.window() {
            window
              .enabled_buttons()
              .contains(winit::window::WindowButtons::MAXIMIZE)
          } else {
            false
          }
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsMinimizable(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          if let Some(window) = w.window() {
            window
              .enabled_buttons()
              .contains(winit::window::WindowButtons::MINIMIZE)
          } else {
            false
          }
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsClosable(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          if let Some(window) = w.window() {
            window
              .enabled_buttons()
              .contains(winit::window::WindowButtons::CLOSE)
          } else {
            false
          }
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::IsVisible(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => {
            Ok(window.is_visible().unwrap_or_default())
          }
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::Title(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => Ok(window.title()),
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::CurrentMonitor(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|w| w.window())
        .and_then(|w| w.current_monitor())
        .map(|monitor| {
          let physical_size = monitor.current_video_mode().map(|vm| vm.size())?;
          let physical_position = monitor.position()?;
          Some(tauri_runtime::monitor::Monitor {
            name: None,
            size: PhysicalSize::new(physical_size.width, physical_size.height),
            position: PhysicalPosition::new(physical_position.x, physical_position.y),
            work_area: tauri_runtime::dpi::PhysicalRect {
              position: PhysicalPosition::new(physical_position.x, physical_position.y),
              size: PhysicalSize::new(physical_size.width, physical_size.height),
            },
            scale_factor: monitor.scale_factor(),
          })
        })
        .map(Ok)
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::PrimaryMonitor(tx) => {
      let result = Ok(crate::monitor::get_primary_monitor());
      let _ = tx.send(result);
    }
    WindowMessage::MonitorFromPoint(tx, x, y) => {
      let result = Ok(crate::monitor::get_monitor_from_point(x, y));
      let _ = tx.send(result);
    }
    WindowMessage::AvailableMonitors(tx) => {
      let monitors = crate::monitor::get_available_monitors();
      let _ = tx.send(Ok(monitors));
    }
    WindowMessage::Theme(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| {
          w.window().and_then(|window| match window.theme() {
            Some(winit::window::Theme::Light) => Some(tauri_utils::Theme::Light),
            Some(winit::window::Theme::Dark) => Some(tauri_utils::Theme::Dark),
            None => None,
          })
        })
        .unwrap_or_default()
        .unwrap_or(tauri_utils::Theme::Light);
      let _ = tx.send(Ok(result));
    }
    WindowMessage::IsEnabled(tx) => {
      let _ = tx.send(Ok(true));
    }
    WindowMessage::IsAlwaysOnTop(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
          crate::window::AppWindowKind::BrowserWindow => {
            Err(tauri_runtime::Error::FailedToSendMessage)
          }
        })
        .unwrap_or_else(|| Err(tauri_runtime::Error::FailedToSendMessage));
      let _ = tx.send(result);
    }
    WindowMessage::RawWindowHandle(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .map(|w| match &w.window {
          crate::window::AppWindowKind::Window(window) => {
            window.window_handle().map(|h| h.as_raw())
          }
          crate::window::AppWindowKind::BrowserWindow => {
            Err(raw_window_handle::HandleError::Unavailable)
          }
        })
        .unwrap_or(Err(raw_window_handle::HandleError::Unavailable));
      let _ = tx.send(result.map(|h| unsafe { WindowHandle::borrow_raw(h) }));
    }
    // Setters
    WindowMessage::Center => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          // TODO:
        }
      }
    }
    WindowMessage::RequestUserAttention(attention_type) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.request_user_attention(match attention_type {
            Some(tauri_runtime::UserAttentionType::Critical) => {
              Some(winit::window::UserAttentionType::Critical)
            }
            Some(tauri_runtime::UserAttentionType::Informational) => {
              Some(winit::window::UserAttentionType::Informational)
            }
            None => None,
          });
        }
      }
    }
    WindowMessage::SetEnabled(_enabled) => {
      // TODO: Implement enabled
    }
    WindowMessage::SetResizable(resizable) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_resizable(resizable);
        }
      }
    }
    WindowMessage::SetMaximizable(maximizable) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          let mut enabled_buttons = window.enabled_buttons();
          enabled_buttons.set(winit::window::WindowButtons::MAXIMIZE, maximizable);
          window.set_enabled_buttons(enabled_buttons);
        }
      }
    }
    WindowMessage::SetMinimizable(minimizable) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          let mut enabled_buttons = window.enabled_buttons();
          enabled_buttons.set(winit::window::WindowButtons::MINIMIZE, minimizable);
          window.set_enabled_buttons(enabled_buttons);
        }
      }
    }
    WindowMessage::SetClosable(closable) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          let mut enabled_buttons = window.enabled_buttons();
          enabled_buttons.set(winit::window::WindowButtons::CLOSE, closable);
          window.set_enabled_buttons(enabled_buttons);
        }
      }
    }
    WindowMessage::SetTitle(title) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_title(&title);
        }
      }
    }
    WindowMessage::Maximize => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_maximized(true);
        }
      }
    }
    WindowMessage::Unmaximize => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_maximized(false);
        }
      }
    }
    WindowMessage::Minimize => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_minimized(true);
        }
      }
    }
    WindowMessage::Unminimize => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_minimized(false);
        }
      }
    }
    WindowMessage::Show => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_visible(true);
        }
      }
    }
    WindowMessage::Hide => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_visible(false);
        }
      }
    }
    WindowMessage::SetDecorations(decorations) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_decorations(decorations);
        }
      }
    }
    WindowMessage::SetShadow(_shadow) => {
      // TODO: Implement shadow
    }
    WindowMessage::SetAlwaysOnBottom(always_on_bottom) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_window_level(if always_on_bottom {
            winit::window::WindowLevel::AlwaysOnBottom
          } else {
            winit::window::WindowLevel::Normal
          });
        }
      }
    }
    WindowMessage::SetAlwaysOnTop(always_on_top) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_window_level(if always_on_top {
            winit::window::WindowLevel::AlwaysOnTop
          } else {
            winit::window::WindowLevel::Normal
          });
        }
      }
    }
    WindowMessage::SetVisibleOnAllWorkspaces(visible_on_all_workspaces) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        // TODO
        // app_window.attributes.borrow_mut().visible_on_all_workspaces =
        //   Some(visible_on_all_workspaces);
      }
      // TODO: Apply visible on all workspaces via platform-specific CEF APIs if available
    }
    WindowMessage::SetContentProtected(protected) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_content_protected(protected);
        }
      }
    }
    WindowMessage::SetSize(size) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.request_surface_size(size);
        }
      }
    }
    WindowMessage::SetMinSize(size) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        // TODO:
        // app_window.attributes.borrow_mut().min_inner_size = size;
      }
    }
    WindowMessage::SetMaxSize(size) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        // TODO:
        // app_window.attributes.borrow_mut().max_inner_size = size;
      }
    }
    WindowMessage::SetSizeConstraints(constraints) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        app_window.attributes.borrow_mut().inner_size_constraints = Some(constraints);
      }
    }
    WindowMessage::SetPosition(position) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_outer_position(position);
        }
      }
    }
    WindowMessage::SetFullscreen(fullscreen) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.set_fullscreen(if fullscreen {
            Some(winit::monitor::Fullscreen::Borderless(None))
          } else {
            None
          });
        }
      }
    }
    #[cfg(target_os = "macos")]
    WindowMessage::SetSimpleFullscreen(_fullscreen) => {
      // TODO: Implement simple fullscreen
    }
    WindowMessage::SetFocus => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.focus_window();
        }
      }
    }
    WindowMessage::SetFocusable(focusable) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          // TODO:
        }
      }
    }
    WindowMessage::SetIcon(icon) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          let icon = winit::icon::RgbaIcon::new(icon.rgba.into(), icon.width, icon.height)
            .expect("failed to create icon from rgba data");
          window.set_window_icon(Some(icon.into()));
        }
      }
    }
    WindowMessage::SetSkipTaskbar(_skip) => {
      // TODO: Implement skip taskbar
    }
    WindowMessage::SetCursorGrab(_grab) => {
      // TODO: Implement cursor grab
    }
    WindowMessage::SetCursorVisible(_visible) => {
      // TODO: Implement cursor visible
    }
    WindowMessage::SetCursorIcon(_icon) => {
      // TODO: Implement cursor icon
    }
    WindowMessage::SetCursorPosition(_position) => {
      // TODO: Implement cursor position
    }
    WindowMessage::SetIgnoreCursorEvents(_ignore) => {
      // TODO: Implement ignore cursor events
    }
    WindowMessage::SetProgressBar(_progress_state) => {
      // TODO: Implement progress bar
    }
    WindowMessage::SetBadgeCount(_count, _desktop_filename) => {
      // TODO: Implement badge count
    }
    WindowMessage::SetBadgeLabel(_label) => {
      // TODO: Implement badge label
    }
    WindowMessage::SetOverlayIcon(icon) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          // TODO:
          // set_overlay_icon(&window, icon);
        }
      }
    }
    WindowMessage::SetTitleBarStyle(_style) => {
      // TODO: Implement title bar style
    }
    WindowMessage::SetTrafficLightPosition(_position) => {
      // TODO: Implement traffic light position
    }
    WindowMessage::SetTheme(_theme) => {
      // TODO: Implement theme
    }
    WindowMessage::SetBackgroundColor(color) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        // TODO
        // app_window.attributes.borrow_mut().background_color = color;
        // if let Some(window) = app_window.window() {
        //   let color_value = color_opt_to_cef_argb(color);
        //   window.set_background_color(color_value);
        // }
      }
    }
    WindowMessage::StartDragging => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(window) = app_window.window() {
          window.drag_window();
        }
      }
    }
    WindowMessage::StartResizeDragging(_direction) => {
      // TODO: Implement start resize dragging
    }
  }
}
