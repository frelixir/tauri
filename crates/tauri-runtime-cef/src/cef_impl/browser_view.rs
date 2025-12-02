use crate::RuntimeStyle as CefRuntimeStyle;
use cef::{rc::*, *};
use std::cell::RefCell;
use std::sync::Arc;

wrap_browser_view_delegate! {
  pub struct CefBrowserViewDelegate {
    browser_id: Arc<RefCell<i32>>,
    browser_runtime_style: CefRuntimeStyle,
  }

  impl ViewDelegate {}

  impl BrowserViewDelegate {
    fn on_browser_created(&self, _browser_view: Option<&mut BrowserView>, browser: Option<&mut Browser>) {
      if let Some(browser) = browser {
        self.browser_id.replace(browser.identifier());
      }
    }

    fn browser_runtime_style(&self) -> RuntimeStyle {
      use cef::sys::cef_runtime_style_t;

      match self.browser_runtime_style {
        CefRuntimeStyle::Alloy => RuntimeStyle::from(cef_runtime_style_t::CEF_RUNTIME_STYLE_ALLOY),
        CefRuntimeStyle::Chrome => RuntimeStyle::from(cef_runtime_style_t::CEF_RUNTIME_STYLE_CHROME),
      }
    }
  }
}
