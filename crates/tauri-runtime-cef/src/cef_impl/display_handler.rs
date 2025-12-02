// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::{rc::*, *};
use std::sync::Arc;

wrap_display_handler! {
  pub struct CefBrowserDisplayHandler {
    document_title_changed_handler: Option<Arc<tauri_runtime::webview::DocumentTitleChangedHandler>>,
  }

  impl DisplayHandler {
    fn on_title_change(
      &self,
      _browser: Option<&mut Browser>,
      title: Option<&CefString>,
    ) {
      let Some(handler) = &self.document_title_changed_handler else { return };
      let Some(title) = title else { return };
      let title_str = title.to_string();
      handler(title_str);
    }
  }
}
