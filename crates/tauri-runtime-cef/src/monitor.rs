use cef::*;
use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize};

/// Convert a CEF Display to a tauri Monitor
pub(crate) fn display_to_monitor(display: &cef::Display) -> tauri_runtime::monitor::Monitor {
  let bounds = display.bounds();
  let work = display.work_area();
  let scale = display.device_scale_factor() as f64;
  let physical_size =
    tauri_runtime::dpi::LogicalSize::new(bounds.width as u32, bounds.height as u32)
      .to_physical::<u32>(scale);
  let physical_position =
    tauri_runtime::dpi::LogicalPosition::new(bounds.x, bounds.y).to_physical::<i32>(scale);
  let work_physical_size =
    tauri_runtime::dpi::LogicalSize::new(work.width as u32, work.height as u32)
      .to_physical::<u32>(scale);
  let work_physical_position =
    tauri_runtime::dpi::LogicalPosition::new(work.x, work.y).to_physical::<i32>(scale);
  tauri_runtime::monitor::Monitor {
    name: None,
    size: PhysicalSize::new(physical_size.width, physical_size.height),
    position: PhysicalPosition::new(physical_position.x, physical_position.y),
    work_area: tauri_runtime::dpi::PhysicalRect {
      position: PhysicalPosition::new(work_physical_position.x, work_physical_position.y),
      size: PhysicalSize::new(work_physical_size.width, work_physical_size.height),
    },
    scale_factor: display.device_scale_factor() as f64,
  }
}

/// Get the primary monitor
pub(crate) fn get_primary_monitor() -> Option<tauri_runtime::monitor::Monitor> {
  cef::display_get_primary().map(|d| display_to_monitor(&d))
}

/// Get the monitor from a point
pub(crate) fn get_monitor_from_point(x: f64, y: f64) -> Option<tauri_runtime::monitor::Monitor> {
  let rect = cef::Rect {
    x: x as i32,
    y: y as i32,
    width: 1,
    height: 1,
  };
  cef::display_get_matching_bounds(Some(&rect), 1).map(|d| display_to_monitor(&d))
}

/// Get all available monitors
pub(crate) fn get_available_monitors() -> Vec<tauri_runtime::monitor::Monitor> {
  let mut displays: Vec<Option<cef::Display>> = vec![None; cef::display_get_count()];
  cef::display_get_alls(Some(&mut displays));
  displays
    .into_iter()
    .flatten()
    .map(|d| display_to_monitor(&d))
    .collect()
}
