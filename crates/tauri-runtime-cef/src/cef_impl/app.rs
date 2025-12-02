// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::{rc::*, *};

wrap_app! {
  pub struct CefApp {
    custom_schemes: Vec<String>,
    command_line_args: Vec<(String, Option<String>)>,
    proxy: winit::event_loop::EventLoopProxy
  }

  impl App {
    fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
      Some(CefBrowserProcessHandler::new(self.proxy.clone()))
    }

    fn on_before_command_line_processing(
      &self,
      _process_type: Option<&CefString>,
      command_line: Option<&mut CommandLine>,
    ) {
      if let Some(command_line) = command_line {
        for (arg, value) in &self.command_line_args {
          if let Some(value) = value {
            command_line.append_switch_with_value(
              Some(&CefString::from(arg.as_str())),
              Some(&CefString::from(value.as_str())),
            );
          } else if arg.starts_with("-") {
            command_line.append_switch(Some(&CefString::from(arg.as_str())));
          } else {
            command_line.append_argument(Some(&CefString::from(arg.as_str())));
          }
        }
      }
    }
  }
}

wrap_browser_process_handler! {
  pub struct CefBrowserProcessHandler {
    proxy: winit::event_loop::EventLoopProxy
  }

  impl BrowserProcessHandler {
    fn on_schedule_message_pump_work(&self, _delay_ms: i64) {
        self.proxy.wake_up();
    }
  }
}
