// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::Arc;

use cef::{rc::*, *};
use tauri_runtime::{window::WindowId, UserEvent};

use super::*;
use crate::{webview::CefInitScript, window::WindowKind};

wrap_client! {
  pub struct CefBrowserClient {
    kind: WindowKind,
    window_id: WindowId,
    initialization_scripts: Arc<Vec<CefInitScript>>,
    on_page_load_handler: Option<Arc<tauri_runtime::webview::OnPageLoadHandler>>,
    document_title_changed_handler: Option<Arc<tauri_runtime::webview::DocumentTitleChangedHandler>>,
    navigation_handler: Option<Arc<tauri_runtime::webview::NavigationHandler>>,
    download_handler: Option<Arc<tauri_runtime::webview::DownloadHandler>>,
    devtools_enabled: bool,
    custom_scheme_domain_names: Vec<String>,
    custom_protocol_scheme: String,
  }

  impl Client {
    fn request_handler(&self) -> Option<RequestHandler> {
      Some(CefWebRequestHandler::new(
        self.initialization_scripts.clone(),
        self.navigation_handler.clone(),
      ))
    }

    fn load_handler(&self) -> Option<LoadHandler> {
      Some(CefBrowserLoadHandler::new(
        self.initialization_scripts.clone(),
        self.on_page_load_handler.clone(),
        self.custom_scheme_domain_names.clone(),
        self.custom_protocol_scheme.clone(),
      ))
    }

    fn display_handler(&self) -> Option<DisplayHandler> {
      if self.document_title_changed_handler.is_some() {
        Some(CefBrowserDisplayHandler::new(
          self.document_title_changed_handler.clone(),
        ))
      } else {
        None
      }
    }

    fn download_handler(&self) -> Option<DownloadHandler> {
      self.download_handler.clone().map(|handler| CefBrowserDownloadHandler::new(handler))
    }

    fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
      Some(CefBrowserContextMenuHandler::new(self.devtools_enabled))
    }

    fn keyboard_handler(&self) -> Option<KeyboardHandler> {
      Some(CefBrowserKeyboardHandler::new(self.devtools_enabled))
    }
  }
}
