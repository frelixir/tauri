// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{cell::Cell, sync::mpsc::channel};

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
  define_class, msg_send,
  rc::Retained,
  runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject},
  ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{NSApp, NSApplication, NSApplicationDelegate, NSApplicationTerminateReply};
use objc2_foundation::{NSArray, NSURL};

pub enum AppDelegateEvent {
  ShouldTerminate {
    tx: std::sync::mpsc::Sender<NSApplicationTerminateReply>,
  },
  OpenURLs {
    urls: Vec<url::Url>,
  },
}

pub struct CefAppDelegateIvars {
  pub on_event: Box<dyn Fn(AppDelegateEvent)>,
}

define_class!(
  #[unsafe(super(NSObject))]
  #[name = "CefAppDelegate"]
  #[ivars = CefAppDelegateIvars]
  #[thread_kind = MainThreadOnly]
  pub struct AppDelegate;

  unsafe impl NSObjectProtocol for AppDelegate {}

  #[allow(non_snake_case)]
  unsafe impl NSApplicationDelegate for AppDelegate {
    #[unsafe(method(application:openURLs:))]
    unsafe fn application_openURLs(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
      let converted_urls: Vec<url::Url> = urls
        .iter()
        .filter_map(|ns_url| {
          ns_url
            .absoluteString()
            .and_then(|url_string| url_string.to_string().parse().ok())
        })
        .collect();

      let handler = &self.ivars().on_event;
      handler(AppDelegateEvent::OpenURLs {
        urls: converted_urls,
      });
    }

    #[unsafe(method(applicationShouldTerminate:))]
    unsafe fn applicationShouldTerminate(
      &self,
      _sender: &NSApplication,
    ) -> NSApplicationTerminateReply {
      let (tx, rx) = channel();
      let handler = &self.ivars().on_event;
      handler(AppDelegateEvent::ShouldTerminate { tx });
      rx.try_recv()
        .unwrap_or(NSApplicationTerminateReply::TerminateNow)
    }
  }
);

impl AppDelegate {
  pub fn new(mtm: MainThreadMarker, on_event: Box<dyn Fn(AppDelegateEvent)>) -> Retained<Self> {
    let delegate = Self::alloc(mtm).set_ivars(CefAppDelegateIvars { on_event });
    let delegate: Retained<Self> = unsafe { msg_send![super(delegate), init] };
    delegate
  }
}

/// Instance variables of `CefSimpleNSApplication`.
pub struct CefSimpleNSApplicationIvars {
  handling_send_event: Cell<Bool>,
}

define_class!(
  /// A `NSApplication` subclass that implements the required CEF protocols.
  ///
  /// This class provides the necessary `CefAppProtocol` conformance to
  /// ensure that events are handled correctly by the Chromium framework on macOS.
  #[unsafe(super(NSApplication))]
  #[ivars = CefSimpleNSApplicationIvars]
  pub struct CefSimpleNSApplication;

  unsafe impl CrAppControlProtocol for CefSimpleNSApplication {
    #[unsafe(method(setHandlingSendEvent:))]
    unsafe fn set_handling_send_event(&self, handling_send_event: Bool) {
      self.ivars().handling_send_event.set(handling_send_event);
    }
  }

  unsafe impl CrAppProtocol for CefSimpleNSApplication {
    #[unsafe(method(isHandlingSendEvent))]
    unsafe fn is_handling_send_event(&self) -> Bool {
      self.ivars().handling_send_event.get()
    }
  }

  unsafe impl CefAppProtocol for CefSimpleNSApplication {}
);

pub fn init_nsapp() {
  let mtm = MainThreadMarker::new().unwrap();

  unsafe {
    // Initialize the SimpleApplication instance.
    // SAFETY: mtm ensures that here is the main thread.
    let _: Retained<AnyObject> = msg_send![CefSimpleNSApplication::class(), sharedApplication];
  }

  // If there was an invocation to NSApp prior to here,
  // then the NSApp will not be a CefSimpleNSApplication.
  // The following assertion ensures that this doesn't happen.
  assert!(NSApp(mtm).isKindOfClass(CefSimpleNSApplication::class()));
}

pub fn init_nsapp_delegate(on_event: Box<dyn Fn(AppDelegateEvent)>) {
  let mtm = MainThreadMarker::new().unwrap();

  let app: Retained<NSApplication> = NSApp(mtm);
  let delegate = AppDelegate::new(mtm, on_event);
  let proto_delegate = ProtocolObject::from_ref(&*delegate);
  app.setDelegate(Some(&proto_delegate));
}
