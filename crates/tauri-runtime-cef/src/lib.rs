// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::*;
use tauri_runtime::{
  dpi::PhysicalPosition,
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowId},
  DeviceEventFilter, Error, ExitRequestedEventAction, Result, RunEvent, RuntimeInitArgs, UserEvent,
};

use tauri_utils::Theme;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
use winit::{
  application::ApplicationHandler,
  event_loop::{EventLoop, EventLoopBuilder},
};

use std::{
  cell::RefCell,
  collections::HashMap,
  fs::create_dir_all,
  sync::{
    atomic::{AtomicU32, Ordering},
    mpsc::{channel, Receiver, Sender},
    Arc, Mutex,
  },
  thread::{self, ThreadId},
};

mod cef_impl;
mod monitor;
#[cfg(target_os = "macos")]
mod nsapplication;
mod webview;
mod window;

#[cfg(target_os = "macos")]
use crate::nsapplication::AppDelegateEvent;
use crate::{cef_impl::*, webview::CefWebviewDispatcher, window::CefWindowDispatcher};
use crate::{
  webview::handle_webview_message,
  window::{handle_window_message, WindowMessage},
};
use crate::{
  webview::{create_webview, WebviewKind, WebviewMessage},
  window::create_window,
};

pub type WebviewId = u32;

#[macro_export]
macro_rules! getter {
  ($self: ident, $rx: expr, $message: expr) => {{
    $self.context.proxy.send_message($message)?;
    $rx
      .recv()
      .map_err(|_| tauri_runtime::Error::FailedToReceiveMessage)
  }};
}

#[macro_export]
macro_rules! webview_getter {
  ($self: ident, $message_variant: path) => {{
    let (tx, rx) = std::sync::mpsc::channel();
    crate::getter!(
      $self,
      rx,
      Message::Webview {
        window_id: *$self.window_id.lock().unwrap(),
        webview_id: $self.webview_id,
        message: $message_variant(tx)
      }
    )
  }};
}

#[macro_export]
macro_rules! window_getter {
  ($self: ident, $message_variant: path) => {{
    let (tx, rx) = std::sync::mpsc::channel();
    crate::getter!(
      $self,
      rx,
      Message::Window {
        window_id: $self.window_id,
        message: $message_variant(tx)
      }
    )
  }};
}

pub enum Message<T: UserEvent + 'static> {
  Task(Box<dyn FnOnce() + Send>),
  CreateWindow {
    window_id: WindowId,
    webview_id: u32,
    pending: PendingWindow<T, CefRuntime<T>>,
    after_window_creation: Option<Box<dyn Fn(RawWindow) + Send + 'static>>,
  },
  CreateWebview {
    window_id: WindowId,
    webview_id: u32,
    pending: PendingWebview<T, CefRuntime<T>>,
  },
  Window {
    window_id: WindowId,
    message: WindowMessage,
  },
  Webview {
    window_id: WindowId,
    webview_id: u32,
    message: WebviewMessage,
  },
  RequestExit(i32),
  UserEvent(T),
}

impl<T: UserEvent> Clone for Message<T> {
  fn clone(&self) -> Self {
    match self {
      Self::UserEvent(t) => Self::UserEvent(t.clone()),
      _ => unimplemented!(),
    }
  }
}

struct WinitApp<T: UserEvent> {
  context: CefRuntimeContext<T>,
  event_rx: Receiver<Message<T>>,
}

impl<T: UserEvent> WinitApp<T> {
  fn new(event_rx: Receiver<Message<T>>, context: CefRuntimeContext<T>) -> Self {
    Self { context, event_rx }
  }
}

impl<T: UserEvent> ApplicationHandler for WinitApp<T> {
  fn new_events(
    &mut self,
    _event_loop: &dyn winit::event_loop::ActiveEventLoop,
    cause: winit::event::StartCause,
  ) {
    if let winit::event::StartCause::Init = cause {
      self.context.run_callback(RunEvent::Ready);
      cef::do_message_loop_work();
    }
  }

  fn resumed(&mut self, _event_loop: &dyn winit::event_loop::ActiveEventLoop) {
    self.context.run_callback(RunEvent::Resumed);
    cef::do_message_loop_work();
  }

  fn can_create_surfaces(&mut self, _event_loop: &dyn winit::event_loop::ActiveEventLoop) {
    cef::do_message_loop_work();
  }

  fn window_event(
    &mut self,
    event_loop: &dyn winit::event_loop::ActiveEventLoop,
    window_id: winit::window::WindowId,
    event: winit::event::WindowEvent,
  ) {
    cef::do_message_loop_work();
  }

  fn proxy_wake_up(&mut self, event_loop: &dyn winit::event_loop::ActiveEventLoop) {
    while let Ok(message) = self.event_rx.try_recv() {
      handle_message(&self.context, event_loop, message);
    }

    cef::do_message_loop_work();
  }
}

/// Platform-specific runtime init attributes.
pub enum RuntimeInitAttribute {
  /// Command line arguments passed to CEF.
  CommandLineArgs { args: Vec<(String, Option<String>)> },
}

/// Webview attributes.
pub enum WebviewAtribute {
  /// Sets the browser runtime style.
  RuntimeStyle { style: RuntimeStyle },
}

/// The browser runtime style.
#[derive(Clone, Copy)]
pub enum RuntimeStyle {
  /// Alloy runtime.
  ///
  /// Used by default on multiwebview mode.
  Alloy,
  /// Chrome runtime.
  ///
  /// Used by default on webview window mode.
  ///
  /// Only a single browser view can use the Chrome runtime in a given window.
  Chrome,
}

#[derive(Clone)]
pub struct CefRuntimeContext<T: UserEvent> {
  pub proxy: EventLoopProxy<T>,
  pub windows: Arc<RefCell<HashMap<WindowId, crate::window::Window>>>,
  pub callback: Arc<RefCell<Option<Box<dyn FnMut(RunEvent<T>)>>>>,
  pub next_window_id: Arc<AtomicU32>,
  pub next_webview_id: Arc<AtomicU32>,
  pub next_window_event_id: Arc<AtomicU32>,
  pub next_webview_event_id: Arc<AtomicU32>,
}

impl<T: UserEvent> std::fmt::Debug for CefRuntimeContext<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("CefRuntimeContext").finish()
  }
}

impl<T: UserEvent> CefRuntimeContext<T> {
  fn new(proxy: EventLoopProxy<T>) -> Self {
    Self {
      callback: Arc::new(RefCell::new(None)),
      proxy,
      windows: Default::default(),
      next_window_id: Arc::new(AtomicU32::new(1)),
      next_webview_id: Arc::new(AtomicU32::new(1)),
      next_window_event_id: Arc::new(AtomicU32::new(1)),
      next_webview_event_id: Arc::new(AtomicU32::new(1)),
    }
  }

  fn next_window_id(&self) -> WindowId {
    self.next_window_id.fetch_add(1, Ordering::Relaxed).into()
  }

  fn next_webview_id(&self) -> WebviewId {
    self.next_webview_id.fetch_add(1, Ordering::Relaxed)
  }

  fn next_window_event_id(&self) -> u32 {
    self.next_window_event_id.fetch_add(1, Ordering::Relaxed)
  }

  fn next_webview_event_id(&self) -> u32 {
    self.next_webview_event_id.fetch_add(1, Ordering::Relaxed)
  }

  fn run_callback(&self, event: RunEvent<T>) {
    if let Some(callback) = self.callback.borrow_mut().as_mut() {
      callback(event);
    }
  }
}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T: UserEvent> Send for CefRuntimeContext<T> {}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T: UserEvent> Sync for CefRuntimeContext<T> {}

#[derive(Debug)]
pub struct CefRuntime<T: UserEvent> {
  event_loop: EventLoop,
  context: CefRuntimeContext<T>,
  event_rx: Receiver<Message<T>>,
}

#[cfg(target_os = "macos")]
fn is_cef_helper_process(exe: &std::path::Path) -> bool {
  const HELPER_SUFFIXES: &[&str] = &[
    " Helper (GPU)",
    " Helper (Renderer)",
    " Helper (Plugin)",
    " Helper (Alerts)",
    " Helper",
  ];

  exe
    .file_name()
    .and_then(|s| s.to_str())
    .map(|name| HELPER_SUFFIXES.iter().any(|s| name.ends_with(s)))
    .unwrap_or_default()
}

impl<T: UserEvent> CefRuntime<T> {
  fn init(runtime_args: RuntimeInitArgs<RuntimeInitAttribute>) -> Self {
    // Initialize NSApplication for CEF on macOS.
    // Must be done before creating the event loop.
    #[cfg(target_os = "macos")]
    nsapplication::init_nsapp();

    let args = cef::args::Args::new();

    // Load CEF framework on macOS and initialize NSApplication
    #[cfg(target_os = "macos")]
    let (_sandbox, _loader) = {
      let exe = std::env::current_exe().unwrap();

      let is_helper = is_cef_helper_process(&exe);

      let sandbox = if is_helper {
        let mut sandbox = cef::sandbox::Sandbox::new();
        sandbox.initialize(args.as_main_args());
        Some(sandbox)
      } else {
        None
      };

      let loader = cef::library_loader::LibraryLoader::new(&exe, is_helper);
      assert!(loader.load());
      (sandbox, loader)
    };

    let mut event_loop_builder = EventLoopBuilder::default();

    #[cfg(windows)]
    if let Some(hook) = runtime_args.msg_hook {
      use winit::platform::windows::EventLoopBuilderExtWindows;
      event_loop_builder.with_msg_hook(hook);
    }

    let event_loop = event_loop_builder
      .build()
      .expect("Failed to create event loop");

    // Initialize NSAppDelegate for macOS
    #[cfg(target_os = "macos")]
    nsapplication::init_nsapp_delegate(Box::new(move |event| match event {
      AppDelegateEvent::ShouldTerminate { tx } => {
        tx.send(objc2_app_kit::NSApplicationTerminateReply::TerminateCancel)
          .unwrap();
        // event_tx_.send(RunEvent::Exit).unwrap();
      }
      AppDelegateEvent::OpenURLs { urls } => {
        // event_tx_.send(RunEvent::Opened { urls }).unwrap();
      }
    }));

    // Set CEF API hash
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);

    // Create CEF cache path
    let cache_base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let cache_path = cache_base.join(&runtime_args.identifier).join("cef");
    let _ = create_dir_all(&cache_path); // Ensure cache directory exists

    // Prepare CEF command line arguments
    let command_line = runtime_args.platform_specific_attributes.into_iter();
    let command_line = command_line
      .filter_map(|a| match a {
        RuntimeInitAttribute::CommandLineArgs { args } => Some(args),
      })
      .flatten()
      .collect::<Vec<_>>();

    // Create CefApp instance
    let mut app = CefApp::new(
      runtime_args.custom_schemes,
      command_line,
      event_loop.create_proxy(),
    );

    // Execute CEF subprocess
    let ret = cef::execute_process(
      Some(args.as_main_args()),
      Some(&mut app),
      std::ptr::null_mut(),
    );

    let cmd = args.as_cmd_line().unwrap();
    let switch = CefString::from("type");
    let is_browser_process = cmd.has_switch(Some(&switch)) != 1;
    if is_browser_process {
      assert!(ret == -1, "cannot execute browser process");
    } else {
      assert!(ret >= 0, "cannot execute non-browser process");
      // non-browser process does not initialize cef
      std::process::exit(0);
    }

    // Initialize CEF
    let settings = cef::Settings {
      no_sandbox: !cfg!(feature = "sandbox") as i32,
      cache_path: cache_path.to_string_lossy().to_string().as_str().into(),
      external_message_pump: 1,
      ..Default::default()
    };
    let ret = cef::initialize(
      Some(args.as_main_args()),
      Some(&settings),
      Some(&mut app),
      std::ptr::null_mut(),
    );
    assert_eq!(ret, 1, "Failed to initialize CEF");

    // Create runtime context
    let (event_tx, event_rx) = channel();
    let proxy = EventLoopProxy {
      tx: event_tx.clone(),
      proxy: event_loop.create_proxy(),
    };

    let context = CefRuntimeContext::new(proxy);

    CefRuntime {
      event_loop,
      context,
      event_rx,
    }
  }
}

impl<T: UserEvent> tauri_runtime::Runtime<T> for CefRuntime<T> {
  type Handle = CefRuntimeHandle<T>;
  type EventLoopProxy = EventLoopProxy<T>;
  type PlatformSpecificInitAttribute = RuntimeInitAttribute;
  type WindowDispatcher = CefWindowDispatcher<T>;
  type WebviewDispatcher = CefWebviewDispatcher<T>;
  type PlatformSpecificWebviewAttribute = WebviewAtribute;

  fn new(args: RuntimeInitArgs<Self::PlatformSpecificInitAttribute>) -> Result<Self> {
    Ok(Self::init(args))
  }

  #[cfg(any(windows, target_os = "linux"))]
  fn new_any_thread(args: RuntimeInitArgs<Self::PlatformSpecificInitAttribute>) -> Result<Self> {
    Ok(Self::init(args))
  }

  fn create_proxy(&self) -> Self::EventLoopProxy {
    self.context.proxy.clone()
  }

  fn handle(&self) -> Self::Handle {
    CefRuntimeHandle {
      context: self.context.clone(),
    }
  }

  fn create_window<F: Fn(RawWindow) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self>> {
    self.context.create_window(pending, after_window_creation)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self>,
  ) -> Result<DetachedWebview<T, Self>> {
    self.context.create_webview(window_id, pending)
  }

  fn primary_monitor(&self) -> Option<Monitor> {
    crate::monitor::get_primary_monitor()
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    crate::monitor::get_monitor_from_point(x, y)
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    crate::monitor::get_available_monitors()
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    Ok(PhysicalPosition::new(0.0, 0.0))
  }

  fn set_theme(&self, theme: Option<Theme>) {}

  #[cfg(target_os = "macos")]
  fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy) {}

  #[cfg(target_os = "macos")]
  fn set_dock_visibility(&mut self, visible: bool) {}

  #[cfg(target_os = "macos")]
  fn show(&self) {}

  #[cfg(target_os = "macos")]
  fn hide(&self) {}

  fn set_device_event_filter(&mut self, filter: DeviceEventFilter) {}

  fn custom_scheme_url(scheme: &str, https: bool) -> String {
    // CEF always uses http/https format regardless of platform
    format!(
      "{}://{scheme}.localhost",
      if https { "https" } else { "http" }
    )
  }

  #[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, _callback: F) {}

  fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, _callback: F) -> i32 {
    0
  }

  fn run<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) {
    let callback = Box::new(callback);
    self.context.callback.borrow_mut().replace(callback);

    let app = WinitApp::new(self.event_rx, self.context.clone());

    self
      .event_loop
      .run_app(app)
      .expect("Failure while running event loop");
  }
}

#[derive(Debug, Clone)]
pub struct CefRuntimeHandle<T: UserEvent> {
  context: CefRuntimeContext<T>,
}

impl<T: UserEvent> tauri_runtime::RuntimeHandle<T> for CefRuntimeHandle<T> {
  type Runtime = CefRuntime<T>;

  fn create_proxy(&self) -> <Self::Runtime as tauri_runtime::Runtime<T>>::EventLoopProxy {
    self.context.proxy.clone()
  }

  #[cfg(target_os = "macos")]
  fn set_activation_policy(
    &self,
    _activation_policy: tauri_runtime::ActivationPolicy,
  ) -> Result<()> {
    Ok(())
  }

  #[cfg(target_os = "macos")]
  fn set_dock_visibility(&self, _visible: bool) -> Result<()> {
    Ok(())
  }

  fn request_exit(&self, code: i32) -> Result<()> {
    // Request exit by posting a task to quit the message loop
    self.context.proxy.send_message(Message::RequestExit(code))
  }

  /// Create a new webview window.
  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self::Runtime>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    self.context.create_window(pending, after_window_creation)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    self.context.create_webview(window_id, pending)
  }

  /// Run a task on the main thread.
  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.proxy.send_message(Message::Task(Box::new(f)))
  }

  fn display_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
    #[cfg(target_os = "linux")]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::Xlib(
        raw_window_handle::XlibDisplayHandle::new(None, 0),
      ))
    });
    #[cfg(target_os = "macos")]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::AppKit(
        raw_window_handle::AppKitDisplayHandle::new(),
      ))
    });
    #[cfg(windows)]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::Windows(
        raw_window_handle::WindowsDisplayHandle::new(),
      ))
    });
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    unimplemented!();
  }

  fn primary_monitor(&self) -> Option<Monitor> {
    crate::monitor::get_primary_monitor()
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    crate::monitor::get_monitor_from_point(x, y)
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    crate::monitor::get_available_monitors()
  }

  fn set_theme(&self, _theme: Option<Theme>) {}

  /// Shows the application, but does not automatically focus it.
  #[cfg(target_os = "macos")]
  fn show(&self) -> Result<()> {
    Ok(())
  }

  /// Hides the application.
  #[cfg(target_os = "macos")]
  fn hide(&self) -> Result<()> {
    Ok(())
  }

  fn set_device_event_filter(&self, _filter: DeviceEventFilter) {}

  #[cfg(target_os = "android")]
  fn find_class<'a>(
    &self,
    env: &mut jni::JNIEnv<'a>,
    activity: &jni::objects::JObject<'_>,
    name: impl Into<String>,
  ) -> std::result::Result<jni::objects::JClass<'a>, jni::errors::Error> {
    todo!()
  }

  #[cfg(target_os = "android")]
  fn run_on_android_context<F>(&self, f: F)
  where
    F: FnOnce(&mut jni::JNIEnv, &jni::objects::JObject, &jni::objects::JObject) + Send + 'static,
  {
    todo!()
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
    &self,
    _cb: F,
  ) -> Result<()> {
    todo!()
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(
    &self,
    _uuid: [u8; 16],
    _cb: F,
  ) -> Result<()> {
    todo!()
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    Ok(PhysicalPosition::new(0.0, 0.0))
  }
}

#[derive(Debug, Clone)]
pub struct EventLoopProxy<T: UserEvent> {
  tx: Sender<Message<T>>,
  proxy: winit::event_loop::EventLoopProxy,
}

// SAFETY: we ensure the context is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T: UserEvent> Send for EventLoopProxy<T> {}

// SAFETY: we ensure the context is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T: UserEvent> Sync for EventLoopProxy<T> {}

impl<T: UserEvent> EventLoopProxy<T> {
  pub fn send_message(&self, message: Message<T>) -> Result<()> {
    let res = self.tx.send(message);
    res.map_err(|_| tauri_runtime::Error::FailedToSendMessage)?;

    self.proxy.wake_up();
    Ok(())
  }
}

impl<T: UserEvent> tauri_runtime::EventLoopProxy<T> for EventLoopProxy<T> {
  fn send_event(&self, event: T) -> Result<()> {
    let message = Message::UserEvent(event);

    let res = self.tx.send(message);
    res.map_err(|_| tauri_runtime::Error::FailedToSendMessage)?;

    self.proxy.wake_up();
    Ok(())
  }
}

pub fn handle_message<T: UserEvent>(
  context: &CefRuntimeContext<T>,
  event_loop: &dyn winit::event_loop::ActiveEventLoop,
  message: Message<T>,
) {
  match message {
    Message::CreateWindow {
      window_id,
      webview_id,
      pending,
      after_window_creation: _todo,
    } => create_window(context, event_loop, window_id, webview_id, pending),
    Message::CreateWebview {
      window_id,
      webview_id,
      pending,
    } => create_webview(
      context,
      WebviewKind::WindowChild,
      window_id,
      webview_id,
      pending,
    ),
    Message::Window { window_id, message } => {
      handle_window_message(context, event_loop, window_id, message);
    }
    Message::Webview {
      window_id,
      webview_id,
      message,
    } => handle_webview_message(context, event_loop, window_id, webview_id, message),
    Message::RequestExit(code) => {
      let (tx, rx) = channel();
      context.run_callback(RunEvent::ExitRequested {
        code: Some(code),
        tx,
      });

      let recv = rx.try_recv();
      let should_prevent = matches!(recv, Ok(ExitRequestedEventAction::Prevent));

      if !should_prevent {
        context.run_callback(RunEvent::Exit);
      }
    }
    Message::Task(t) => t(),
    Message::UserEvent(evt) => {
      context.run_callback(RunEvent::UserEvent(evt));
    }
  }
}
