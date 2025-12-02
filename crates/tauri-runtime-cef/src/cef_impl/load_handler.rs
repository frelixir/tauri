// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use cef::{rc::*, *};
use std::sync::Arc;

use crate::webview::CefInitScript;

wrap_load_handler! {
  pub struct CefBrowserLoadHandler {
    initialization_scripts: Arc<Vec<CefInitScript>>,
    on_page_load_handler: Option<Arc<tauri_runtime::webview::OnPageLoadHandler>>,
    custom_scheme_domain_names: Vec<String>,
    custom_protocol_scheme: String,
  }

  impl LoadHandler {
    fn on_load_start(
      &self,
      _browser: Option<&mut Browser>,
      frame: Option<&mut Frame>,
      _transition_type: TransitionType,
    ) {
      let Some(handler) = &self.on_page_load_handler else { return };
      let Some(frame) = frame else { return };

      let is_main_frame = frame.is_main() == 1;
      if !is_main_frame {
        return;
      }

      let url = frame.url();
      let url_str = cef::CefString::from(&url).to_string();
      if let Ok(url) = url::Url::parse(&url_str) {
        handler(url, tauri_runtime::webview::PageLoadEvent::Started);
      }
    }

    fn on_load_end(
      &self,
      _browser: Option<&mut Browser>,
      frame: Option<&mut Frame>,
      http_status_code: ::std::os::raw::c_int,
    ) {
      let Some(frame) = frame else { return };

      if let Some(handler) = &self.on_page_load_handler {
        if frame.is_main() == 1 {
          let url = frame.url();
          let url_str = cef::CefString::from(&url).to_string();
          if let Ok(url) = url::Url::parse(&url_str) {
            handler(url, tauri_runtime::webview::PageLoadEvent::Finished);
          }
        }
      }

      // run init scripts for http/https pages that are not custom schemes
      // custom schemes are handled by the request handler
      // where we inject scripts directly in the html

      if !(200..300).contains(&http_status_code) {
        return;
      }

      let url = frame.url();
      let url_str = cef::CefString::from(&url).to_string();
      let url_obj = url::Url::parse(&url_str).ok();

      let is_custom_scheme_url = url_obj
        .as_ref()
        .map(|u| {
          let scheme = u.scheme();
          if scheme == self.custom_protocol_scheme {
            let host_str = u.host_str().unwrap_or("").to_string();
            scheme == self.custom_protocol_scheme && self.custom_scheme_domain_names.contains(&host_str)
          } else {
            false
          }
        });
      // if we can't parse the URL, also return
      if is_custom_scheme_url.unwrap_or(true) { return; }

      let is_main_frame = frame.is_main() == 1;

      let scripts_to_execute = if is_main_frame {
       Box::new(self.initialization_scripts.iter().map(|s| &s.script.script)) as Box<dyn std::iter::Iterator<Item = &String>>
      } else {
        Box::new(self.initialization_scripts
          .iter()
          .filter(|s| !s.script.for_main_frame_only)
          .map(|s| &s.script.script)) as Box<dyn std::iter::Iterator<Item = &String>>
      };

      for script in scripts_to_execute {
        let script_url = format!("{}://__tauri_init_script__", url_obj.as_ref().map(|u| u.scheme()).unwrap_or("http"));

        frame.execute_java_script(
          Some(&cef::CefString::from(script.as_str())),
          Some(&cef::CefString::from(script_url.as_str())),
          0,
        );
      }
    }
  }
}
