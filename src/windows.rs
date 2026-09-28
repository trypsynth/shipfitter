//! The Windows application manifest and version resource, for a build script. Both do nothing
//! when the target isn't Windows.

use std::env;

use embed_manifest::{
	manifest::{
		ActiveCodePage, DpiAwareness, HeapType, Setting,
		SupportedOS::{Windows7, Windows10},
	},
	new_manifest,
};
use winres::WindowsResource;

use crate::Result;

fn targets_windows() -> bool {
	env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
}

/// Embeds a manifest asking for version 6 of the common controls, UTF-8 as the code page, the
/// segment heap, per-monitor DPI awareness and long paths.
pub fn embed_manifest(app_name: &str) -> Result<()> {
	if !targets_windows() {
		return Ok(());
	}
	let manifest = new_manifest(app_name)
		.supported_os(Windows7..=Windows10)
		.active_code_page(ActiveCodePage::Utf8)
		.heap_type(HeapType::SegmentHeap)
		.dpi_awareness(DpiAwareness::PerMonitorV2)
		.long_path_aware(Setting::Enabled);
	embed_manifest::embed_manifest(manifest)?;
	Ok(())
}

/// What Explorer shows in the executable's properties.
#[derive(Debug, Clone, Default)]
pub struct VersionInfo<'a> {
	pub product_name: &'a str,
	pub company: &'a str,
	pub copyright: &'a str,
	/// The executable's file name, such as `fedra.exe`.
	pub original_filename: &'a str,
	/// Shown as the product version in place of the crate's, such as `0.7.0 (abc1234)` for a
	/// development build.
	pub product_version: Option<&'a str>,
	/// An `.ico` file, relative to the crate, that Explorer, the taskbar and file associations
	/// show for the executable.
	pub icon: Option<&'a str>,
}

impl VersionInfo<'_> {
	pub fn embed(&self) -> Result<()> {
		if !targets_windows() {
			return Ok(());
		}
		let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
		let mut resource = WindowsResource::new();
		if let Some(icon) = self.icon {
			resource.set_icon(icon);
		}
		resource
			.set("ProductName", self.product_name)
			.set("FileDescription", self.product_name)
			.set("LegalCopyright", self.copyright)
			.set("CompanyName", self.company)
			.set("OriginalFilename", self.original_filename)
			.set("ProductVersion", self.product_version.unwrap_or(&version))
			.set("FileVersion", &version);
		resource.compile()?;
		Ok(())
	}
}
