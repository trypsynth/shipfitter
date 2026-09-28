//! Build and packaging helpers shared by Rust desktop apps.
//!
//! [`build`] and [`macos`] are always available and need nothing beyond the standard library, so
//! a build script can use them. The `package` feature adds archives and installers for an xtask,
//! and `windows-resources` adds the Windows manifest and version resource for a build script.

pub mod build;
pub mod macos;
#[cfg(feature = "package")]
pub mod package;
#[cfg(feature = "windows-resources")]
pub mod windows;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The suffix release assets use for an architecture as Rust names it: `arm64` for `aarch64`,
/// `x64` for `x86_64`, and anything else unchanged.
#[must_use]
pub fn arch_suffix(arch: &str) -> &str {
	match arch {
		"aarch64" => "arm64",
		"x86_64" => "x64",
		other => other,
	}
}

/// [`arch_suffix`] for the machine running this code, which is what an xtask wants.
#[must_use]
pub fn host_arch_suffix() -> &'static str {
	arch_suffix(std::env::consts::ARCH)
}
