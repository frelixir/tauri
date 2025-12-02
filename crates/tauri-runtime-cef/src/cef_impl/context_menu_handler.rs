// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::{rc::*, *};

wrap_context_menu_handler! {
  pub struct CefBrowserContextMenuHandler {
    devtools_enabled: bool,
  }

  impl ContextMenuHandler {
    fn on_before_context_menu(
      &self,
      _browser: Option<&mut Browser>,
      _frame: Option<&mut Frame>,
      _params: Option<&mut ContextMenuParams>,
      model: Option<&mut MenuModel>,
    ) {
      if !self.devtools_enabled {
        if let Some(model) = model {
          model.remove_at(model.count() - 1);
        }
      }
    }
  }
}
