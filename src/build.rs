//! Helpers for build scripts.

use std::{
	env, fs,
	path::{Path, PathBuf},
	process::Command,
};

use crate::{Result, arch_suffix};

/// The profile directory the binary is linked into, such as `target/release`. Build scripts put
/// generated files there so they sit next to the executable.
#[must_use]
pub fn target_profile_dir() -> Option<PathBuf> {
	let profile = env::var("PROFILE").ok()?;
	if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
		return Some(PathBuf::from(target_dir).join(profile));
	}
	let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
	out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

/// [`arch_suffix`] for the architecture being built for.
#[must_use]
pub fn target_arch_suffix() -> String {
	arch_suffix(&env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default()).to_string()
}

/// The commit being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
	/// The full hash, or `unknown` outside a Git checkout.
	pub hash: String,
	/// The first seven characters of the hash.
	pub short_hash: String,
	/// Whether the commit has no tag, so it's a development build rather than a release.
	pub is_dev: bool,
}

fn git(args: &[&str]) -> Option<String> {
	let output = Command::new("git").args(args).output().ok()?;
	output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[must_use]
pub fn commit_info() -> CommitInfo {
	let hash = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
	let short_hash = hash.chars().take(7).collect();
	let is_dev = git(&["describe", "--tags", "--exact-match", "HEAD"]).is_none();
	CommitInfo { hash, short_hash, is_dev }
}

/// Sets `{prefix}_COMMIT_HASH`, `{prefix}_SHORT_HASH` and `{prefix}_IS_DEV` (`1` or `0`) for
/// `env!`, and reruns the build script when the checked out commit changes.
pub fn embed_commit_info(prefix: &str) -> CommitInfo {
	let info = commit_info();
	println!("cargo:rustc-env={prefix}_COMMIT_HASH={}", info.hash);
	println!("cargo:rustc-env={prefix}_SHORT_HASH={}", info.short_hash);
	println!("cargo:rustc-env={prefix}_IS_DEV={}", u8::from(info.is_dev));
	if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) {
		println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
		println!("cargo:rerun-if-changed={}", git_dir.join("packed-refs").display());
		if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
			println!("cargo:rerun-if-changed={}", git_dir.join(head_ref).display());
		}
	}
	info
}

#[must_use]
pub fn pandoc_available() -> bool {
	Command::new("pandoc").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Converts `input` with pandoc using the options in the `defaults` file.
///
/// Reruns the build script when either changes. `lang` sets the document language, as a BCP 47
/// tag such as `pt-BR`, so screen readers read the result in the right voice.
pub fn pandoc(input: &Path, defaults: &Path, output: &Path, lang: Option<&str>) -> Result<()> {
	println!("cargo:rerun-if-changed={}", input.display());
	println!("cargo:rerun-if-changed={}", defaults.display());
	let mut command = Command::new("pandoc");
	command.arg(format!("--defaults={}", defaults.display()));
	if let Some(lang) = lang {
		command.args(["-M", &format!("lang={lang}")]);
	}
	let status = command.arg(input).arg("-o").arg(output).status()?;
	if !status.success() {
		return Err(format!("pandoc failed to convert {}", input.display()).into());
	}
	Ok(())
}

/// Copies `template` to `output`, replacing each `@NAME@` with its value.
///
/// This works like `configure_file` in `CMake`, and reruns the build script when the template
/// changes.
///
/// These are always filled in, before `replacements`:
///
/// - `@PROJECT_VERSION@`: the crate's version.
/// - `@ARCH_SUFFIX@`: the [`target_arch_suffix`].
/// - `@ARCH_ISS@`, `@ARCH_ALLOWED@` and `@ARCH_MODE@`: the Inno Setup architecture identifier,
///   `arm64` or `x64compatible`, for `ArchitecturesAllowed` and
///   `ArchitecturesInstallIn64BitMode`.
pub fn configure_file(template: &Path, output: &Path, replacements: &[(&str, &str)]) -> Result<()> {
	println!("cargo:rerun-if-changed={}", template.display());
	let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
	let arch_suffix = target_arch_suffix();
	let arch_iss = if arch_suffix == "arm64" { "arm64" } else { "x64compatible" };
	let standard = [
		("PROJECT_VERSION", version.as_str()),
		("ARCH_SUFFIX", arch_suffix.as_str()),
		("ARCH_ISS", arch_iss),
		("ARCH_ALLOWED", arch_iss),
		("ARCH_MODE", arch_iss),
	];
	let mut content = fs::read_to_string(template)?;
	for (name, value) in standard.iter().chain(replacements) {
		content = content.replace(&format!("@{name}@"), value);
	}
	fs::write(output, content)?;
	Ok(())
}
