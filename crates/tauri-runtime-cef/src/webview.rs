use base64::Engine;
use cef::*;
use sha2::{Digest, Sha256};
use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size},
  webview::{
    DetachedWebview, InitializationScript, PendingWebview, UriSchemeProtocol, WebviewAttributes,
  },
  window::{WebviewEvent, WindowId},
  Cookie, Result, UserEvent, WebviewEventId,
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;
use tauri_utils::{config::Color, html::normalize_script_for_csp};
use url::Url;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use std::{
  cell::RefCell,
  collections::HashMap,
  sync::{atomic::AtomicBool, mpsc::Sender, Arc, Mutex},
};

use crate::{
  cef_impl::{
    CefBrowserClient, CefBrowserViewDelegate, CefCollectAllCookiesVisitor,
    CefCollectUrlCookiesVisitor, CefUriSchemeHandlerFactory,
  },
  webview_getter,
  window::{CefWindowBuilder, WindowKind},
  Message, WebviewAtribute,
};
use crate::{CefRuntime, CefRuntimeContext, RuntimeStyle as CefRuntimeStyle};

pub type WebviewId = u32;

#[derive(Clone)]
pub struct CefInitScript {
  pub script: InitializationScript,
  pub hash: String,
}

impl CefInitScript {
  pub fn new(script: InitializationScript) -> Self {
    let hash = Self::hash_script(script.script.as_str());
    Self { script, hash }
  }

  fn hash_script(script: &str) -> String {
    let normalized = normalize_script_for_csp(script.as_bytes());
    let mut hasher = Sha256::new();
    hasher.update(&normalized);
    let hash = hasher.finalize();
    format!(
      "'sha256-{}'",
      base64::engine::general_purpose::STANDARD.encode(hash)
    )
  }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum WebviewKind {
  // webview is the entire window content
  WindowContent,
  // webview is a child of the window, which can contain other webviews too
  WindowChild,
}

#[derive(Debug, Clone)]
pub struct WebviewBounds {
  pub x_rate: f32,
  pub y_rate: f32,
  pub width_rate: f32,
  pub height_rate: f32,
}

#[derive(Clone)]
pub struct Webview {
  pub webview_id: u32,
  pub label: String,
  pub browser: Option<cef::Browser>,
  // browser_view.browser is null on the scheme handler factory,
  // so we need to use the browser_id to identify the browser
  pub browser_id: Arc<RefCell<i32>>,
  pub overlay: Option<cef::OverlayController>,
  pub bounds: Arc<Mutex<Option<WebviewBounds>>>,
  pub devtools_enabled: bool,
  pub uri_scheme_protocols:
    Arc<HashMap<String, Arc<Box<tauri_runtime::webview::UriSchemeProtocol>>>>,
  pub initialization_scripts: Arc<Vec<CefInitScript>>,
}

impl std::fmt::Debug for Webview {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Webview")
      .field("webview_id", &self.webview_id)
      .field("label", &self.label)
      .finish()
  }
}

pub enum WebviewMessage {
  AddEventListener(WebviewEventId, Box<dyn Fn(&WebviewEvent) + Send>),
  EvaluateScript(String),
  CookiesForUrl(Url, Sender<Result<Vec<Cookie<'static>>>>),
  Cookies(Sender<Result<Vec<Cookie<'static>>>>),
  SetCookie(Cookie<'static>),
  DeleteCookie(Cookie<'static>),
  Navigate(Url),
  Reload,
  Print,
  Close,
  Show,
  Hide,
  SetPosition(Position),
  SetSize(Size),
  SetBounds(Rect),
  SetFocus,
  Reparent(WindowId, Sender<Result<()>>),
  SetAutoResize(bool),
  SetZoom(f64),
  SetBackgroundColor(Option<Color>),
  ClearAllBrowsingData,
  // Getters
  Url(Sender<Result<String>>),
  Bounds(Sender<Result<Rect>>),
  Position(Sender<Result<PhysicalPosition<i32>>>),
  Size(Sender<Result<PhysicalSize<u32>>>),
  WithWebview(Box<dyn FnOnce(Box<dyn std::any::Any>) + Send>),
  // Devtools
  #[cfg(any(debug_assertions, feature = "devtools"))]
  OpenDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  CloseDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  IsDevToolsOpen(Sender<bool>),
}

#[derive(Debug, Clone)]
pub struct CefWebviewDispatcher<T: UserEvent> {
  pub window_id: Arc<Mutex<WindowId>>,
  pub webview_id: u32,
  pub context: CefRuntimeContext<T>,
}

impl<T: UserEvent> tauri_runtime::WebviewDispatch<T> for CefWebviewDispatcher<T> {
  type Runtime = CefRuntime<T>;

  fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::WithWebview(Box::new(f)),
    })
  }

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.proxy.send_message(Message::Task(Box::new(f)))
  }

  fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(&self, f: F) -> WebviewEventId {
    let id = self.context.next_webview_event_id();
    let _ = self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::AddEventListener(id, Box::new(f)),
    });
    id
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn open_devtools(&self) {
    let _ = self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::OpenDevTools,
    });
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn close_devtools(&self) {
    let _ = self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::CloseDevTools,
    });
  }

  #[cfg(any(debug_assertions, feature = "devtools"))]
  fn is_devtools_open(&self) -> Result<bool> {
    webview_getter!(self, WebviewMessage::IsDevToolsOpen)
  }

  fn set_zoom(&self, scale_factor: f64) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetZoom(scale_factor),
    })
  }

  fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::EvaluateScript(script.into()),
    })
  }

  fn url(&self) -> Result<String> {
    webview_getter!(self, WebviewMessage::Url)?
  }

  fn bounds(&self) -> Result<Rect> {
    webview_getter!(self, WebviewMessage::Bounds)?
  }

  fn position(&self) -> Result<PhysicalPosition<i32>> {
    webview_getter!(self, WebviewMessage::Position)?
  }

  fn size(&self) -> Result<PhysicalSize<u32>> {
    webview_getter!(self, WebviewMessage::Size)?
  }

  fn navigate(&self, url: Url) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Navigate(url),
    })
  }

  fn reload(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Reload,
    })
  }

  fn print(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Print,
    })
  }

  fn close(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Close,
    })
  }

  fn set_bounds(&self, bounds: Rect) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetBounds(bounds),
    })
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetSize(size),
    })
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetPosition(position),
    })
  }

  fn set_focus(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetFocus,
    })
  }

  fn reparent(&self, window_id: WindowId) -> Result<()> {
    let mut current_window_id = self.window_id.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    self.context.proxy.send_message(Message::Webview {
      window_id: *current_window_id,
      webview_id: self.webview_id,
      message: WebviewMessage::Reparent(window_id, tx),
    })?;

    rx.recv().unwrap()?;

    *current_window_id = window_id;
    Ok(())
  }

  fn cookies_for_url(&self, url: Url) -> Result<Vec<Cookie<'static>>> {
    let current_window_id = self.window_id.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    self.context.proxy.send_message(Message::Webview {
      window_id: *current_window_id,
      webview_id: self.webview_id,
      message: WebviewMessage::CookiesForUrl(url, tx),
    })?;

    rx.recv().unwrap()
  }

  fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
    webview_getter!(self, WebviewMessage::Cookies)?
  }

  fn set_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetCookie(cookie.into_owned()),
    })
  }

  fn delete_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::DeleteCookie(cookie.into_owned()),
    })
  }

  fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetAutoResize(auto_resize),
    })
  }

  fn clear_all_browsing_data(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::ClearAllBrowsingData,
    })
  }

  fn hide(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Hide,
    })
  }

  fn show(&self) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::Show,
    })
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    self.context.proxy.send_message(Message::Webview {
      window_id: *self.window_id.lock().unwrap(),
      webview_id: self.webview_id,
      message: WebviewMessage::SetBackgroundColor(color),
    })
  }
}

impl<T: UserEvent> CefRuntimeContext<T> {
  pub fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, CefRuntime<T>>,
  ) -> Result<DetachedWebview<T, CefRuntime<T>>> {
    let label = pending.label.clone();
    let webview_id = self.next_webview_id();

    crate::webview::create_webview(
      &self,
      crate::webview::WebviewKind::WindowChild,
      window_id,
      webview_id,
      pending,
    );

    let dispatcher = CefWebviewDispatcher {
      window_id: Arc::new(Mutex::new(window_id)),
      webview_id,
      context: self.clone(),
    };

    Ok(DetachedWebview { label, dispatcher })
  }
}

#[inline]
fn color_to_cef_argb(color: tauri_utils::config::Color) -> u32 {
  let (r, g, b, a) = color.into();
  ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
fn color_opt_to_cef_argb(color: Option<tauri_utils::config::Color>) -> u32 {
  color.map(color_to_cef_argb).unwrap_or(0xFFFFFFFF)
}

pub fn create_browser_window<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  window_id: WindowId,
  webview_id: u32,
  label: String,
  window_builder: CefWindowBuilder,
  webview: PendingWebview<T, CefRuntime<T>>,
) {
  let PendingWebview {
    label: webview_label,
    mut webview_attributes,
    platform_specific_attributes: _,
    uri_scheme_protocols,
    ipc_handler: _,
    navigation_handler,
    new_window_handler: _,
    document_title_changed_handler,
    url,
    web_resource_request_handler: _,
    mut on_page_load_handler,
    download_handler,
  } = webview;

  let initialization_scripts = std::mem::take(&mut webview_attributes.initialization_scripts)
    .into_iter()
    .map(CefInitScript::new)
    .collect::<Vec<_>>();
  let initialization_scripts = Arc::new(initialization_scripts);

  let on_page_load_handler = on_page_load_handler.take().map(Arc::from);
  let document_title_changed_handler = document_title_changed_handler.map(Arc::from);
  let navigation_handler = navigation_handler.map(Arc::from);

  let devtools_enabled = (cfg!(debug_assertions) || cfg!(feature = "devtools"))
    && webview_attributes.devtools.unwrap_or(true);

  let custom_protocol_scheme = if webview_attributes.use_https_scheme {
    "https"
  } else {
    "http"
  };

  // Build cached domain names for custom schemes and clone protocols for storage
  // before uri_scheme_protocols is moved
  let scheme_keys: Vec<String> = uri_scheme_protocols.keys().cloned().collect();
  let custom_scheme_domain_names: Vec<String> = scheme_keys
    .iter()
    .map(|scheme| format!("{scheme}.localhost"))
    .collect();

  let uri_scheme_protocols: HashMap<String, Arc<Box<UriSchemeProtocol>>> = uri_scheme_protocols
    .into_iter()
    .map(|(k, v)| (k, Arc::new(v)))
    .collect();

  let custom_schemes = uri_scheme_protocols.keys().cloned().collect::<Vec<_>>();

  let mut request_context = request_context_from_webview_attributes(
    context,
    &webview_attributes,
    &custom_schemes,
    custom_protocol_scheme,
    &initialization_scripts,
  );

  let browser_settings = browser_settings_from_webview_attributes(&webview_attributes);

  // Create the AppWindow with BrowserWindow variant before creating the browser
  let force_close = Arc::new(AtomicBool::new(false));
  let attributes = Arc::new(RefCell::new(window_builder));

  let mut client = CefBrowserClient::new(
    WindowKind::Browser,
    window_id,
    initialization_scripts.clone(),
    on_page_load_handler,
    document_title_changed_handler,
    navigation_handler,
    download_handler,
    devtools_enabled,
    custom_scheme_domain_names.clone(),
    custom_protocol_scheme.to_string(),
  );

  let url = CefString::from(url.as_str());

  let mut bounds = cef::Rect {
    x: 0,
    y: 0,
    width: 800,
    height: 600,
  };
  let device_scale_factor = display_get_primary()
    .map(|d| d.device_scale_factor() as f64)
    .unwrap_or(1.);
  if let Some(size) = attributes.borrow().attrs.surface_size {
    let size = size.to_logical::<i32>(device_scale_factor);
    bounds.width = size.width;
    bounds.height = size.height;
  }
  if let Some(position) = attributes.borrow().attrs.position {
    let position = position.to_logical::<i32>(device_scale_factor);
    bounds.x = position.x;
    bounds.y = position.y;
  }

  let window_info = WindowInfo {
    ..Default::default()
  };

  let Some(browser) = browser_host_create_browser_sync(
    Some(&window_info),
    Some(&mut client),
    Some(&url),
    Some(&browser_settings),
    None,
    request_context.as_mut(),
  ) else {
    eprintln!("Failed to create browser");
    return;
  };

  context.windows.borrow_mut().insert(
    window_id,
    crate::window::Window {
      label,
      window: crate::window::AppWindowKind::BrowserWindow,
      force_close: force_close.clone(),
      attributes: attributes.clone(),
      webviews: vec![Webview {
        webview_id,
        browser_id: Arc::new(RefCell::new(browser.identifier())),
        label: webview_label,
        browser: None,
        overlay: None,
        bounds: Arc::new(Mutex::new(None)),
        devtools_enabled,
        uri_scheme_protocols: Arc::new(uri_scheme_protocols),
        initialization_scripts,
      }],
      window_event_listeners: Arc::new(Mutex::new(HashMap::new())),
      webview_event_listeners: Arc::new(Mutex::new(HashMap::new())),
    },
  );
}

pub(crate) fn create_webview<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  kind: WebviewKind,
  window_id: WindowId,
  webview_id: u32,
  pending: PendingWebview<T, CefRuntime<T>>,
) {
  let PendingWebview {
    label,
    mut webview_attributes,
    platform_specific_attributes,
    uri_scheme_protocols,
    ipc_handler: _,
    navigation_handler,
    new_window_handler: _,
    document_title_changed_handler,
    url,
    web_resource_request_handler: _,
    mut on_page_load_handler,
    download_handler,
  } = pending;

  let window = match context
    .windows
    .borrow()
    .get(&window_id)
    .and_then(|app_window| app_window.window())
  {
    Some(w) => w,
    None => {
      eprintln!("Window {window_id:?} not found or is a browser window when creating webview",);
      return;
    }
  };

  let initialization_scripts = std::mem::take(&mut webview_attributes.initialization_scripts)
    .into_iter()
    .map(CefInitScript::new)
    .collect::<Vec<_>>();
  let initialization_scripts = Arc::new(initialization_scripts);

  let on_page_load_handler = on_page_load_handler.take().map(Arc::from);
  let document_title_changed_handler = document_title_changed_handler.map(Arc::from);
  let navigation_handler = navigation_handler.map(Arc::from);

  let devtools_enabled = (cfg!(debug_assertions) || cfg!(feature = "devtools"))
    && webview_attributes.devtools.unwrap_or(true);

  let custom_protocol_scheme = if webview_attributes.use_https_scheme {
    "https"
  } else {
    "http"
  };

  let custom_schemes = uri_scheme_protocols.keys().cloned().collect::<Vec<_>>();
  let custom_scheme_domain_names: Vec<String> = custom_schemes
    .iter()
    .map(|scheme| format!("{scheme}.localhost"))
    .collect();

  let mut client = CefBrowserClient::new(
    WindowKind::Tauri,
    window_id,
    initialization_scripts.clone(),
    on_page_load_handler,
    document_title_changed_handler,
    navigation_handler,
    download_handler,
    devtools_enabled,
    custom_scheme_domain_names.clone(),
    custom_protocol_scheme.to_string(),
  );
  let url = CefString::from(url.as_str());

  let uri_scheme_protocols: HashMap<String, Arc<Box<UriSchemeProtocol>>> = uri_scheme_protocols
    .into_iter()
    .map(|(k, v)| (k, Arc::new(v)))
    .collect();

  let mut request_context = request_context_from_webview_attributes(
    context,
    &webview_attributes,
    &custom_schemes,
    custom_protocol_scheme,
    &initialization_scripts,
  );

  let browser_id = Arc::new(RefCell::new(0));
  let mut browser_view_delegate = CefBrowserViewDelegate::new(
    browser_id.clone(),
    platform_specific_attributes
      .iter()
      .find_map(|attr| match attr {
        WebviewAtribute::RuntimeStyle { style } => Some(*style),
      })
      .unwrap_or(if matches!(kind, WebviewKind::WindowChild) {
        CefRuntimeStyle::Alloy
      } else {
        CefRuntimeStyle::Chrome
      }),
  );

  let browser_settings = browser_settings_from_webview_attributes(&webview_attributes);

  let mut window_info = cef::WindowInfo::default();

  let Ok(handle) = window.window_handle() else {
    return;
  };
  let handle = handle.as_raw();

  match handle {
    #[cfg(target_os = "macos")]
    RawWindowHandle::AppKit(handle) => {
      window_info.parent_view = handle.ns_view.as_ptr();
    }
    #[cfg(target_os = "linux")]
    RawWindowHandle::Xlib(handle) => {
      window_info.parent_window = handle.window;
    }
    _ => {
      return;
    }
  }

  let size = window.surface_size();

  window_info.bounds = cef::Rect {
    x: 0,
    y: 0,
    width: size.width as i32,
    height: size.height as i32,
  };

  let browser = browser_host_create_browser_sync(
    Some(&window_info),
    Some(&mut client),
    Some(&url),
    Some(&browser_settings),
    Option::<&mut DictionaryValue>::None,
    request_context.as_mut(),
  )
  .expect("Failed to create browser view");

  *browser_id.borrow_mut() = browser.identifier();

  context
    .windows
    .borrow_mut()
    .get_mut(&window_id)
    .unwrap()
    .webviews
    .push(Webview {
      label,
      webview_id,
      browser: Some(browser),
      browser_id,
      overlay: None,
      bounds: Arc::new(Mutex::new(None)),
      devtools_enabled,
      uri_scheme_protocols: Arc::new(uri_scheme_protocols),
      initialization_scripts,
    });
}

fn webview_bounds_ratio(
  window: &cef::Window,
  webview_bounds: Option<cef::Rect>,
  overlay: &OverlayController,
) -> crate::webview::WebviewBounds {
  let window_bounds = window.bounds();
  let window_size =
    tauri_runtime::dpi::LogicalSize::new(window_bounds.width as u32, window_bounds.height as u32);

  let ob = webview_bounds.unwrap_or_else(|| overlay.bounds());
  let pos = tauri_runtime::dpi::LogicalPosition::new(ob.x, ob.y);
  let size = tauri_runtime::dpi::LogicalSize::new(ob.width as u32, ob.height as u32);

  crate::webview::WebviewBounds {
    x_rate: pos.x as f32 / window_size.width as f32,
    y_rate: pos.y as f32 / window_size.height as f32,
    width_rate: size.width as f32 / window_size.width as f32,
    height_rate: size.height as f32 / window_size.height as f32,
  }
}

fn browser_settings_from_webview_attributes(
  webview_attributes: &WebviewAttributes,
) -> BrowserSettings {
  BrowserSettings {
    javascript: State::from(if webview_attributes.javascript_disabled {
      sys::cef_state_t::STATE_DISABLED
    } else {
      sys::cef_state_t::STATE_ENABLED
    }),
    javascript_access_clipboard: State::from(if webview_attributes.clipboard {
      sys::cef_state_t::STATE_ENABLED
    } else {
      sys::cef_state_t::STATE_DISABLED
    }),
    ..Default::default()
  }
}

fn request_context_from_webview_attributes<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  webview_attributes: &WebviewAttributes,
  custom_schemes: &[String],
  custom_protocol_scheme: &str,
  _initialization_scripts: &[CefInitScript],
) -> Option<RequestContext> {
  let global_context =
    request_context_get_global_context().expect("Failed to get global request context");

  let cache_path: CefStringUtf16 = if webview_attributes.incognito {
    CefStringUtf16::from("")
  } else if let Some(_data_directory) = &webview_attributes.data_directory {
    // TODO: setting a custom data directory must be a child of the root data directory, but it returns None on browser_view_create
    eprintln!("data directory is not yet implemented");
    (&global_context.cache_path()).into()
    // CefStringUtf16::from(data_directory.to_string_lossy().as_ref())
  } else {
    (&global_context.cache_path()).into()
  };

  let request_context_settings = RequestContextSettings {
    cache_path,
    ..Default::default()
  };

  let request_context = request_context_create_context(
    Some(&request_context_settings),
    Option::<&mut RequestContextHandler>::None,
  );
  if let Some(request_context) = &request_context {
    for custom_scheme in custom_schemes {
      request_context.register_scheme_handler_factory(
        Some(&custom_protocol_scheme.into()),
        Some(&format!("{custom_scheme}.localhost").as_str().into()),
        Some(&mut CefUriSchemeHandlerFactory::new(
          context.clone(),
          custom_scheme.clone(),
        )),
      );
    }
  }

  request_context
}

#[cfg(target_os = "macos")]
pub fn macos_webview_bounds(
  window: &dyn winit::window::Window,
  mut bounds: cef::Rect,
) -> cef::Rect {
  bounds.y += window_titlebar_height(window);
  bounds
}

#[cfg(target_os = "macos")]
pub fn window_titlebar_height(window: &dyn winit::window::Window) -> i32 {
  use objc2::rc::Retained;
  use objc2_app_kit::NSView;
  use raw_window_handle::{HasWindowHandle, RawWindowHandle};

  let Ok(handle) = window.window_handle() else {
    return 0;
  };
  let handle = handle.as_raw();
  let RawWindowHandle::AppKit(handle) = handle else {
    return 0;
  };

  unsafe {
    let Some(content_view) = Retained::<NSView>::retain(handle.ns_view.as_ptr() as _) else {
      return 0;
    };
    let Some(ns_window) = content_view.window() else {
      return 0;
    };
    let content_layout_rect = ns_window.contentLayoutRect();
    let window_bounds = window.surface_size();
    let titlebar_height = window_bounds.height as f64 - content_layout_rect.size.height;
    titlebar_height as i32
  }
}

#[cfg(target_os = "macos")]
fn apply_titlebar_style(window: &cef::Window, style: TitleBarStyle) {
  use objc2::rc::Retained;
  use objc2_app_kit::{NSView, NSWindowStyleMask};

  let content_view = unsafe { Retained::<NSView>::retain(window.window_handle() as _) };
  let Some(content_view) = content_view else {
    return;
  };

  let Some(ns_window) = content_view.window() else {
    return;
  };

  let mut mask = ns_window.styleMask();

  match style {
    TitleBarStyle::Visible => {
      mask |= NSWindowStyleMask::FullSizeContentView;
      ns_window.setTitlebarAppearsTransparent(false);
      ns_window.setStyleMask(mask);
    }
    TitleBarStyle::Transparent => {
      ns_window.setTitlebarAppearsTransparent(true);
      mask &= !NSWindowStyleMask::FullSizeContentView;
      ns_window.setStyleMask(mask);
    }
    TitleBarStyle::Overlay => {
      ns_window.setTitlebarAppearsTransparent(true);
      mask |= NSWindowStyleMask::FullSizeContentView;
      ns_window.setStyleMask(mask);
    }
    unknown => {
      eprintln!("unknown title bar style applied: {unknown}");
    }
  }
}

fn get_browser<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  window_id: WindowId,
  webview_id: u32,
) -> Option<cef::Browser> {
  context
    .windows
    .borrow()
    .get(&window_id)
    .and_then(|app_window| {
      app_window
        .webviews
        .iter()
        .find(|w| w.webview_id == webview_id)
        .and_then(|w| w.browser.clone())
    })
}

fn get_webview<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  window_id: WindowId,
  webview_id: u32,
) -> Option<Webview> {
  context
    .windows
    .borrow()
    .get(&window_id)
    .and_then(|app_window| {
      app_window
        .webviews
        .iter()
        .find(|w| w.webview_id == webview_id)
        .cloned()
    })
}

pub fn handle_webview_message<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  window_id: WindowId,
  webview_id: u32,
  message: WebviewMessage,
) {
  match message {
    WebviewMessage::AddEventListener(event_id, handler) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        let listeners = app_window.webview_event_listeners.clone();
        let mut listeners_map = listeners.lock().unwrap();
        let webview_listeners = listeners_map
          .entry(webview_id)
          .or_insert_with(|| Arc::new(Mutex::new(HashMap::new())));
        webview_listeners.lock().unwrap().insert(event_id, handler);
      }
    }
    WebviewMessage::EvaluateScript(script) => {
      if let Some(frame) = get_browser(context, window_id, webview_id).and_then(|b| b.main_frame())
      {
        frame.execute_java_script(
          Some(&cef::CefString::from(script.as_str())),
          Some(&cef::CefString::from("")),
          0,
        );
      }
    }
    WebviewMessage::Navigate(url) => {
      if let Some(frame) = get_browser(context, window_id, webview_id).and_then(|b| b.main_frame())
      {
        frame.load_url(Some(&cef::CefString::from(url.as_str())))
      }
    }
    WebviewMessage::Reload => {
      if let Some(browser) = get_browser(context, window_id, webview_id) {
        browser.reload()
      }
    }
    WebviewMessage::Print => {
      if let Some(host) = get_browser(context, window_id, webview_id).and_then(|b| b.host()) {
        host.print()
      }
    }
    WebviewMessage::Close => {
      if let Some(app_window) = context.windows.borrow_mut().get_mut(&window_id) {
        let webview_index = app_window
          .webviews
          .iter()
          .position(|w| w.webview_id == webview_id);

        if let Some(index) = webview_index {
          let browser_view_wrapper = app_window.webviews.remove(index);

          if let Some(overlay) = browser_view_wrapper.overlay {
            overlay.destroy();
          }

          app_window
            .webview_event_listeners
            .lock()
            .unwrap()
            .remove(&webview_id);
        }
      }
    }
    WebviewMessage::Show => {
      if let Some(overlay) = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
        })
        .and_then(|wrapper| wrapper.overlay.as_ref())
      {
        overlay.set_visible(1)
      }
    }
    WebviewMessage::Hide => {
      if let Some(overlay) = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
        })
        .and_then(|wrapper| wrapper.overlay.as_ref())
      {
        overlay.set_visible(0)
      }
    }
    WebviewMessage::SetPosition(position) => {
      context.windows.borrow().get(&window_id).map(|app_window| {
        let device_scale_factor = app_window
          .window()
          .map(|d| d.scale_factor() as f64)
          .unwrap_or(1.0);
        let logical_position = position.to_logical::<i32>(device_scale_factor);
        app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
          .and_then(|wrapper| wrapper.overlay.as_ref())
          .map(|overlay| {
            let current_bounds = overlay.bounds();
            let new_bounds = cef::Rect {
              x: logical_position.x,
              y: logical_position.y,
              width: current_bounds.width,
              height: current_bounds.height,
            };
            #[cfg(target_os = "macos")]
            let new_bounds = if let Some(window) = app_window.window() {
              macos_webview_bounds(&**window, new_bounds)
            } else {
              new_bounds
            };
            overlay.set_bounds(Some(&new_bounds));
          });

        // update autoresize ratios if enabled
        if let Some(wrapper) = app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
        {
          if wrapper.overlay.is_some() {
            if let Some(b) = &mut *wrapper.bounds.lock().unwrap() {
              if let Some(window) = app_window.window() {
                let window_bounds = window.surface_size();
                let window_size = tauri_runtime::dpi::LogicalSize::new(
                  window_bounds.width as u32,
                  window_bounds.height as u32,
                );

                let pos =
                  tauri_runtime::dpi::LogicalPosition::new(logical_position.x, logical_position.y);
                b.x_rate = pos.x as f32 / window_size.width as f32;
                b.y_rate = pos.y as f32 / window_size.height as f32;
              }
            }
          }
        }
      });
    }
    WebviewMessage::SetSize(size) => {
      context.windows.borrow().get(&window_id).map(|app_window| {
        let device_scale_factor = app_window
          .window()
          .map(|d| d.scale_factor() as f64)
          .unwrap_or(1.0);
        let logical_size = size.to_logical::<u32>(device_scale_factor);
        app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
          .and_then(|wrapper| wrapper.overlay.as_ref())
          .map(|overlay| {
            let current_bounds = overlay.bounds();
            let new_bounds = cef::Rect {
              x: current_bounds.x,
              y: current_bounds.y,
              width: logical_size.width as i32,
              height: logical_size.height as i32,
            };
            #[cfg(target_os = "macos")]
            let new_bounds = if let Some(window) = app_window.window() {
              macos_webview_bounds(&**window, new_bounds)
            } else {
              new_bounds
            };
            overlay.set_bounds(Some(&new_bounds));
          });

        // update autoresize ratios if enabled
        if let Some(wrapper) = app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
        {
          if wrapper.overlay.is_some() {
            if let Some(b) = &mut *wrapper.bounds.lock().unwrap() {
              if let Some(window) = app_window.window() {
                let window_bounds = window.surface_size();
                let window_size = tauri_runtime::dpi::LogicalSize::new(
                  window_bounds.width as u32,
                  window_bounds.height as u32,
                );

                let s =
                  tauri_runtime::dpi::LogicalSize::new(logical_size.width, logical_size.height);
                b.width_rate = s.width as f32 / window_size.width as f32;
                b.height_rate = s.height as f32 / window_size.height as f32;
              }
            }
          }
        }
      });
    }
    WebviewMessage::SetBounds(bounds) => {
      context.windows.borrow().get(&window_id).map(|app_window| {
        let device_scale_factor = app_window
          .window()
          .map(|d| d.scale_factor() as f64)
          .unwrap_or(1.0);
        let logical_position = bounds.position.to_logical::<i32>(device_scale_factor);
        let logical_size = bounds.size.to_logical::<u32>(device_scale_factor);
        if let Some(overlay) = app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
          .and_then(|wrapper| wrapper.overlay.as_ref())
        {
          let bounds = cef::Rect {
            x: logical_position.x,
            y: logical_position.y,
            width: logical_size.width as i32,
            height: logical_size.height as i32,
          };
          #[cfg(target_os = "macos")]
          let bounds = if let Some(window) = app_window.window() {
            macos_webview_bounds(&**window, bounds)
          } else {
            bounds
          };
          overlay.set_bounds(Some(&bounds));
        }

        // update autoresize ratios if enabled
        if let Some(wrapper) = app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
        {
          if wrapper.overlay.is_some() {
            if let Some(b) = &mut *wrapper.bounds.lock().unwrap() {
              if let Some(window) = app_window.window() {
                let window_bounds = window.surface_size();
                let window_size = tauri_runtime::dpi::LogicalSize::new(
                  window_bounds.width as u32,
                  window_bounds.height as u32,
                );

                let pos =
                  tauri_runtime::dpi::LogicalPosition::new(logical_position.x, logical_position.y);
                let s =
                  tauri_runtime::dpi::LogicalSize::new(logical_size.width, logical_size.height);
                b.x_rate = pos.x as f32 / window_size.width as f32;
                b.y_rate = pos.y as f32 / window_size.height as f32;
                b.width_rate = s.width as f32 / window_size.width as f32;
                b.height_rate = s.height as f32 / window_size.height as f32;
              }
            }
          }
        }
      });
    }
    WebviewMessage::SetFocus => {
      if let Some(host) = get_browser(context, window_id, webview_id).and_then(|b| b.host()) {
        host.set_focus(1)
      }
    }
    WebviewMessage::Reparent(target_window_id, tx) => {
      // TODO
    }
    WebviewMessage::SetAutoResize(auto_resize) => {
      if let Some(app_window) = context.windows.borrow().get(&window_id) {
        if let Some(wrapper) = app_window
          .webviews
          .iter()
          .find(|w| w.webview_id == webview_id)
        {
          if let Some(overlay) = &wrapper.overlay {
            if auto_resize {
              if let Some(window) = app_window.window() {
                let window_bounds = window.surface_size();
                let window_size = tauri_runtime::dpi::LogicalSize::new(
                  window_bounds.width as u32,
                  window_bounds.height as u32,
                );

                let ob = overlay.bounds();
                let pos = tauri_runtime::dpi::LogicalPosition::new(ob.x, ob.y);
                let size = tauri_runtime::dpi::LogicalSize::new(ob.width as u32, ob.height as u32);

                *wrapper.bounds.lock().unwrap() = Some(crate::webview::WebviewBounds {
                  x_rate: pos.x as f32 / window_size.width as f32,
                  y_rate: pos.y as f32 / window_size.height as f32,
                  width_rate: size.width as f32 / window_size.width as f32,
                  height_rate: size.height as f32 / window_size.height as f32,
                });
              }
            } else {
              *wrapper.bounds.lock().unwrap() = None;
            }
          }
        }
      }
    }
    WebviewMessage::SetZoom(scale_factor) => {
      if let Some(host) = get_browser(context, window_id, webview_id).and_then(|b| b.host()) {
        host.set_zoom_level(scale_factor)
      }
    }
    WebviewMessage::SetBackgroundColor(color) => {
      let color_value = color_opt_to_cef_argb(color);
      if let Some(bv) = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
        })
        .and_then(|wrapper| wrapper.browser.as_ref())
      {
        // TODO:
        // bv.set_background_color(color_value)
      }
    }
    WebviewMessage::ClearAllBrowsingData => {
      // TODO: Implement clear browsing data
    }
    // Getters
    WebviewMessage::Url(tx) => {
      let result = get_browser(context, window_id, webview_id)
        .and_then(|b| b.main_frame())
        .map(|frame| {
          let url = frame.url();
          cef::CefString::from(&url).to_string()
        })
        .ok_or(tauri_runtime::Error::FailedToSendMessage);
      let _ = tx.send(result);
    }
    WebviewMessage::Bounds(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
            .and_then(|webview| {
              let bounds_opt = webview
                .overlay
                .as_ref()
                .map(|overlay| overlay.bounds())
                .or_else(|| {
                  let bounds = match &app_window.window {
                    crate::window::AppWindowKind::Window(window) => {
                      let size = window.surface_size();
                      let pos = window
                        .outer_position()
                        .expect("Failed to get window position");
                      cef::Rect {
                        x: pos.x,
                        y: pos.y,
                        width: size.width as i32,
                        height: size.height as i32,
                      }
                    }
                    crate::window::AppWindowKind::BrowserWindow => {
                      todo!("Get bounds from browser window")
                    }
                  };
                  Some(bounds)
                });
              bounds_opt.map(|bounds| {
                let scale = match &app_window.window {
                  crate::window::AppWindowKind::Window(window) => window.scale_factor(),
                  crate::window::AppWindowKind::BrowserWindow => 1.0,
                };
                let logical_position = tauri_runtime::dpi::LogicalPosition::new(bounds.x, bounds.y);
                let logical_size =
                  tauri_runtime::dpi::LogicalSize::new(bounds.width as u32, bounds.height as u32);
                let physical_position = logical_position.to_physical::<i32>(scale);
                let physical_size = logical_size.to_physical::<u32>(scale);
                Rect {
                  position: Position::Physical(physical_position),
                  size: Size::Physical(physical_size),
                }
              })
            })
        })
        .ok_or(tauri_runtime::Error::FailedToSendMessage);
      let _ = tx.send(result);
    }
    WebviewMessage::Position(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
            .and_then(|webview| {
              let bounds = webview.overlay.as_ref().map(|v| v.bounds()).or_else(|| {
                let bounds = match &app_window.window {
                  crate::window::AppWindowKind::Window(window) => {
                    let size = window.surface_size();
                    let pos = window
                      .outer_position()
                      .expect("Failed to get window position");
                    cef::Rect {
                      x: pos.x,
                      y: pos.y,
                      width: size.width as i32,
                      height: size.height as i32,
                    }
                  }
                  crate::window::AppWindowKind::BrowserWindow => {
                    todo!("Get bounds from browser window")
                  }
                };
                Some(bounds)
              })?;
              let scale = match &app_window.window {
                crate::window::AppWindowKind::Window(window) => window.scale_factor(),
                crate::window::AppWindowKind::BrowserWindow => 1.0,
              };
              Some(
                tauri_runtime::dpi::LogicalPosition::new(bounds.x, bounds.y)
                  .to_physical::<i32>(scale),
              )
            })
        })
        .ok_or(tauri_runtime::Error::FailedToSendMessage);
      let _ = tx.send(result);
    }
    WebviewMessage::Size(tx) => {
      let result = context
        .windows
        .borrow()
        .get(&window_id)
        .and_then(|app_window| {
          app_window
            .webviews
            .iter()
            .find(|w| w.webview_id == webview_id)
            .and_then(|webview| {
              let Some(bounds) = webview.overlay.as_ref().map(|v| v.bounds()).or_else(|| {
                let bounds = match &app_window.window {
                  crate::window::AppWindowKind::Window(window) => {
                    let size = window.surface_size();
                    let pos = window
                      .outer_position()
                      .expect("Failed to get window position");
                    cef::Rect {
                      x: pos.x,
                      y: pos.y,
                      width: size.width as i32,
                      height: size.height as i32,
                    }
                  }
                  crate::window::AppWindowKind::BrowserWindow => {
                    todo!("Get bounds from browser window")
                  }
                };
                Some(bounds)
              }) else {
                return None;
              };
              let scale = match &app_window.window {
                crate::window::AppWindowKind::Window(window) => window.scale_factor(),
                crate::window::AppWindowKind::BrowserWindow => 1.0,
              };
              Some(
                tauri_runtime::dpi::LogicalSize::new(bounds.width as u32, bounds.height as u32)
                  .to_physical::<u32>(scale),
              )
            })
        })
        .ok_or(tauri_runtime::Error::FailedToSendMessage);
      let _ = tx.send(result);
    }
    WebviewMessage::WithWebview(f) => {
      if let Some(browser_view) = get_browser(context, window_id, webview_id) {
        f(Box::new(browser_view));
      }
    }
    // Devtools
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::OpenDevTools => {
      get_webview(context, window_id, webview_id)
        .and_then(|bv| {
          if bv.devtools_enabled {
            bv.browser
          } else {
            // break out of the chain if devtools are not enabled
            None
          }
        })
        .and_then(|b| b.host())
        .map(|host| {
          let window_info = cef::WindowInfo::default();
          let settings = cef::BrowserSettings::default();
          let inspect_at = cef::Point { x: 0, y: 0 };
          host.show_dev_tools(
            Some(&window_info),
            Option::<&mut cef::Client>::None,
            Some(&settings),
            Some(&inspect_at),
          );
        });
    }
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::CloseDevTools => {
      if let Some(host) = get_browser(context, window_id, webview_id).and_then(|b| b.host()) {
        host.close_dev_tools()
      }
    }
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::IsDevToolsOpen(tx) => {
      let _ = tx.send(false);
    }
    WebviewMessage::CookiesForUrl(url, tx) => {
      // Collect cookies for a specific URL
      let url_str = url.as_str().to_string();

      cef::cookie_manager_get_global_manager(None)
        .map(|manager| {
          let collected: Arc<Mutex<Vec<tauri_runtime::Cookie<'static>>>> =
            Arc::new(Mutex::new(Vec::new()));
          let tx_ = tx.clone();

          let mut visitor = CefCollectUrlCookiesVisitor::new(tx_, collected.clone());
          let url_cef = cef::CefString::from(url_str.as_str());
          manager.visit_url_cookies(Some(&url_cef), 1, Some(&mut visitor));
        })
        .or_else(|| {
          let _ = tx.send(Ok(Vec::new()));
          None
        });
    }
    WebviewMessage::Cookies(tx) => {
      // Collect all cookies
      cef::cookie_manager_get_global_manager(None)
        .map(|manager| {
          let collected: Arc<Mutex<Vec<tauri_runtime::Cookie<'static>>>> =
            Arc::new(Mutex::new(Vec::new()));
          let tx_ = tx.clone();

          let mut visitor = CefCollectAllCookiesVisitor::new(tx_, collected.clone());
          manager.visit_all_cookies(Some(&mut visitor));
        })
        .or_else(|| {
          let _ = tx.send(Ok(Vec::new()));
          None
        });
    }
    WebviewMessage::SetCookie(cookie) => {
      if let Some(manager) = cef::cookie_manager_get_global_manager(None) {
        // Try to infer a URL for the cookie scope using the currently loaded URL
        let url = get_browser(context, window_id, webview_id)
          .and_then(|b| b.main_frame())
          .map(|frame| cef::CefString::from(&frame.url()).to_string())
          .unwrap_or_default();

        let mut cef_cookie = cef::Cookie::default();
        cef_cookie.name = cef::CefString::from(cookie.name());
        cef_cookie.value = cef::CefString::from(cookie.value());
        if let Some(d) = cookie.domain() {
          cef_cookie.domain = cef::CefString::from(d);
        }
        if let Some(p) = cookie.path() {
          cef_cookie.path = cef::CefString::from(p);
        }
        if cookie.secure().unwrap_or(false) {
          cef_cookie.secure = 1;
        }
        if cookie.http_only().unwrap_or(false) {
          cef_cookie.httponly = 1;
        }

        let url_cef = if url.is_empty() {
          None
        } else {
          Some(cef::CefString::from(url.as_str()))
        };
        manager.set_cookie(
          url_cef.as_ref(),
          Some(&cef_cookie),
          Option::<&mut cef::SetCookieCallback>::None,
        );
      }
    }
    WebviewMessage::DeleteCookie(cookie) => {
      if let Some(manager) = cef::cookie_manager_get_global_manager(None) {
        // Resolve current URL for targeted deletion
        let url = get_browser(context, window_id, webview_id)
          .and_then(|b| b.main_frame())
          .map(|frame| cef::CefString::from(&frame.url()).to_string())
          .unwrap_or_default();
        let url_cef = if url.is_empty() {
          None
        } else {
          Some(cef::CefString::from(url.as_str()))
        };
        let name_cef = Some(cef::CefString::from(cookie.name()));
        manager.delete_cookies(
          url_cef.as_ref(),
          name_cef.as_ref(),
          Option::<&mut cef::DeleteCookiesCallback>::None,
        );
      }
    }
  }
}
