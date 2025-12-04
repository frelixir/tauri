use crate::RuntimeStyle as CefRuntimeStyle;
use cef::{rc::*, *};

wrap_browser_view_delegate! {
  pub struct CefBrowserViewDelegate {
    browser_runtime_style: CefRuntimeStyle,
  }

  impl ViewDelegate {}

  impl BrowserViewDelegate {
    fn browser_runtime_style(&self) -> RuntimeStyle {
      use cef::sys::cef_runtime_style_t;

      match self.browser_runtime_style {
        CefRuntimeStyle::Alloy => RuntimeStyle::from(cef_runtime_style_t::CEF_RUNTIME_STYLE_ALLOY),
        CefRuntimeStyle::Chrome => RuntimeStyle::from(cef_runtime_style_t::CEF_RUNTIME_STYLE_CHROME),
      }
    }
  }
}
