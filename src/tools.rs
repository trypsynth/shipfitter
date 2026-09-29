//! The tools packaging runs, found where they're installed or else downloaded once into a cache
//! shared by every project, so building a release needs nothing installed beyond Rust.
//!
//! Each download is a pinned release checked against its SHA-256 before it's used.

use std::{
	env,
	fmt::Write as _,
	fs::{self, File},
	io::{self, Read, Write},
	path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::Result;

const INNO_SETUP_VERSION: &str = "7.1.0";
const INNO_SETUP_URL: &str = "https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe";
const INNO_SETUP_SHA256: &str = "0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f";

const APPIMAGETOOL_VERSION: &str = "1.9.1";
const APPIMAGETOOL_X86_64_SHA256: &str = "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0";
const APPIMAGETOOL_AARCH64_SHA256: &str = "f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158";

/// Where downloaded tools are kept.
///
/// That's `SHIPFITTER_CACHE_DIR` if it's set, and otherwise a `shipfitter` folder in the user's cache directory (`%LOCALAPPDATA%` on Windows,
/// `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere). It's outside
/// `target`, so `cargo clean` doesn't throw the tools away.
pub fn cache_dir() -> Result<PathBuf> {
	if let Some(dir) = env::var_os("SHIPFITTER_CACHE_DIR") {
		return Ok(PathBuf::from(dir));
	}
	let base = if cfg!(windows) {
		env::var_os("LOCALAPPDATA").map(PathBuf::from)
	} else if cfg!(target_os = "macos") {
		env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
	} else {
		env::var_os("XDG_CACHE_HOME")
			.map(PathBuf::from)
			.or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
	};
	Ok(base.ok_or("couldn't find a cache directory; set SHIPFITTER_CACHE_DIR")?.join("shipfitter"))
}

/// The Inno Setup command-line compiler, `ISCC.exe`.
///
/// That's `ISCC` if it's set, then `PATH`, then the standard Inno Setup 7 and 6 installs, and otherwise Inno Setup 7 unpacked into the
/// [`cache_dir`]. Unpacking it is a portable install for the current user, so it needs no
/// administrator rights and leaves nothing in the registry.
pub fn iscc() -> Result<PathBuf> {
	if let Some(path) = env::var_os("ISCC") {
		return Ok(PathBuf::from(path));
	}
	if !cfg!(windows) {
		return Err("Inno Setup only runs on Windows".into());
	}
	let installed = [("ProgramFiles", "Inno Setup 7"), ("ProgramFiles(x86)", "Inno Setup 6")]
		.into_iter()
		.filter_map(|(var, dir)| env::var_os(var).map(|base| PathBuf::from(base).join(dir).join("ISCC.exe")));
	if let Some(path) = find_on_path("ISCC.exe").into_iter().chain(installed).find(|path| path.is_file()) {
		return Ok(path);
	}
	let cache = cache_dir()?;
	let dir = cache.join(format!("inno-setup-{INNO_SETUP_VERSION}"));
	let iscc = dir.join("ISCC.exe");
	if iscc.is_file() {
		return Ok(iscc);
	}
	println!("Downloading Inno Setup {INNO_SETUP_VERSION} into {}", cache.display());
	let setup_exe = cache.join(format!("innosetup-{INNO_SETUP_VERSION}-{}.exe", std::process::id()));
	download(INNO_SETUP_URL, INNO_SETUP_SHA256, &setup_exe)?;
	// Unpack beside the final folder and rename it into place, so a build that's interrupted, or
	// another one running at the same time, never sees half an install.
	let staging = cache.join(format!("inno-setup-{INNO_SETUP_VERSION}.{}", std::process::id()));
	let status = std::process::Command::new(&setup_exe)
		.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER", "/PORTABLE=1", "/NOICONS"])
		.arg(format!("/DIR={}", staging.display()))
		.status();
	let _ = fs::remove_file(&setup_exe);
	if !status?.success() || !staging.join("ISCC.exe").is_file() {
		let _ = fs::remove_dir_all(&staging);
		return Err("the Inno Setup installer failed to unpack".into());
	}
	if fs::rename(&staging, &dir).is_err() {
		// Another build got there first.
		let _ = fs::remove_dir_all(&staging);
	}
	Ok(iscc)
}

/// `appimagetool`: `APPIMAGETOOL` if it's set, then `PATH`, and otherwise the release for this
/// machine's architecture downloaded into the [`cache_dir`].
pub fn appimagetool() -> Result<PathBuf> {
	if let Some(path) = env::var_os("APPIMAGETOOL") {
		return Ok(PathBuf::from(path));
	}
	if let Some(path) = find_on_path("appimagetool") {
		return Ok(path);
	}
	let (arch, sha256) = match env::consts::ARCH {
		"x86_64" => ("x86_64", APPIMAGETOOL_X86_64_SHA256),
		"aarch64" => ("aarch64", APPIMAGETOOL_AARCH64_SHA256),
		other => return Err(format!("no appimagetool download for {other}; set APPIMAGETOOL").into()),
	};
	let cache = cache_dir()?;
	let tool = cache.join(format!("appimagetool-{APPIMAGETOOL_VERSION}-{arch}.AppImage"));
	if !tool.is_file() {
		println!("Downloading appimagetool {APPIMAGETOOL_VERSION} into {}", cache.display());
		let url = format!(
			"https://github.com/AppImage/appimagetool/releases/download/{APPIMAGETOOL_VERSION}/appimagetool-{arch}.AppImage"
		);
		download(&url, sha256, &tool)?;
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))?;
	}
	Ok(tool)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
	env::split_paths(&env::var_os("PATH")?).map(|dir| dir.join(name)).find(|path| path.is_file())
}

/// Downloads `url` to `dest`, failing unless its SHA-256 is `sha256`. It's written beside `dest`
/// first and renamed into place once it checks out, so `dest` is never a partial download.
fn download(url: &str, sha256: &str, dest: &Path) -> Result<()> {
	if let Some(parent) = dest.parent() {
		fs::create_dir_all(parent)?;
	}
	let partial = dest.with_extension(format!("part{}", std::process::id()));
	let result = (|| -> Result<()> {
		let mut reader = ureq::get(url).call()?.into_body().into_reader();
		let mut file = File::create(&partial)?;
		let mut hasher = Sha256::new();
		let mut buffer = vec![0; 64 * 1024];
		loop {
			let read = match reader.read(&mut buffer) {
				Ok(0) => break,
				Ok(read) => read,
				Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
				Err(err) => return Err(err.into()),
			};
			hasher.update(&buffer[..read]);
			file.write_all(&buffer[..read])?;
		}
		file.flush()?;
		let actual = hasher.finalize().iter().fold(String::new(), |mut hex, byte| {
			let _ = write!(hex, "{byte:02x}");
			hex
		});
		if actual != sha256 {
			return Err(format!("{url} has SHA-256 {actual}, expected {sha256}").into());
		}
		Ok(())
	})();
	if let Err(err) = result {
		let _ = fs::remove_file(&partial);
		return Err(format!("couldn't download {url}: {err}").into());
	}
	fs::rename(&partial, dest)?;
	Ok(())
}
