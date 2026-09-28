//! Release builds, archives and installers, for an xtask.

use std::{
	env,
	fs::{self, File},
	io::{self, Write},
	path::{Path, PathBuf},
	process::Command,
};

use walkdir::WalkDir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::Result;

/// Builds `packages` in release mode from the workspace at `root`, or the default members when
/// `packages` is empty.
///
/// Each package gets a Cargo invocation of its own: when several are built in one, Cargo hands
/// every one of them the native library paths any of their build scripts asked for, so a command
/// line tool could end up importing the GUI libraries another package links.
pub fn cargo_build_release(root: &Path, packages: &[&str]) -> Result<()> {
	let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
	let build = |package: Option<&str>| -> Result<()> {
		let mut command = Command::new(&cargo);
		command.current_dir(root).args(["build", "--release"]);
		if let Some(package) = package {
			command.args(["-p", package]);
		}
		if !command.status()?.success() {
			return Err(format!("cargo build failed for {}", package.unwrap_or("the workspace")).into());
		}
		Ok(())
	};
	if packages.is_empty() {
		return build(None);
	}
	packages.iter().try_for_each(|package| build(Some(package)))
}

/// The version of `package` in the workspace at `root`, as `cargo metadata` resolves it, so a
/// version inherited from the workspace is found too.
pub fn crate_version(root: &Path, package: &str) -> Result<String> {
	let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
	let output =
		Command::new(cargo).args(["metadata", "--format-version", "1", "--no-deps"]).current_dir(root).output()?;
	if !output.status.success() {
		return Err("cargo metadata failed".into());
	}
	let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
	metadata["packages"]
		.as_array()
		.and_then(|packages| packages.iter().find(|p| p["name"] == package))
		.and_then(|p| p["version"].as_str())
		.map(str::to_string)
		.ok_or_else(|| format!("cargo metadata has no package named {package}").into())
}

/// A zip archive being written.
pub struct Zip {
	writer: ZipWriter<File>,
	options: SimpleFileOptions,
}

impl Zip {
	pub fn create(path: &Path) -> Result<Self> {
		let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
		Ok(Self { writer: ZipWriter::new(File::create(path)?), options })
	}

	/// Adds the file at `path` as `name`.
	pub fn file(&mut self, path: &Path, name: &str) -> Result<&mut Self> {
		self.writer.start_file(name, self.options)?;
		io::copy(&mut File::open(path)?, &mut self.writer)?;
		Ok(self)
	}

	/// Adds the folder at `path` and everything in it as `name`.
	pub fn dir(&mut self, path: &Path, name: &str) -> Result<&mut Self> {
		for entry in WalkDir::new(path) {
			let entry = entry?;
			let relative = entry.path().strip_prefix(path)?.to_string_lossy().replace('\\', "/");
			let entry_name = if relative.is_empty() { name.to_string() } else { format!("{name}/{relative}") };
			if entry.file_type().is_dir() {
				self.writer.add_directory(entry_name, self.options)?;
			} else {
				self.file(entry.path(), &entry_name)?;
			}
		}
		Ok(self)
	}

	pub fn finish(self) -> Result<()> {
		self.writer.finish()?.flush()?;
		Ok(())
	}
}

/// A gzipped tarball being written.
pub struct TarGz {
	builder: tar::Builder<flate2::write::GzEncoder<File>>,
}

impl TarGz {
	pub fn create(path: &Path) -> Result<Self> {
		let encoder = flate2::write::GzEncoder::new(File::create(path)?, flate2::Compression::default());
		Ok(Self { builder: tar::Builder::new(encoder) })
	}

	/// Adds the file at `path` as `name`, keeping its permissions.
	pub fn file(&mut self, path: &Path, name: &str) -> Result<&mut Self> {
		self.builder.append_path_with_name(path, name)?;
		Ok(self)
	}

	/// Adds the folder at `path` and everything in it as `name`.
	pub fn dir(&mut self, path: &Path, name: &str) -> Result<&mut Self> {
		self.builder.append_dir_all(name, path)?;
		Ok(self)
	}

	pub fn finish(self) -> Result<()> {
		self.builder.into_inner()?.finish()?;
		Ok(())
	}
}

/// What goes in an `AppImage`.
#[derive(Debug, Clone)]
pub struct AppImage<'a> {
	/// Executables and the libraries beside them, copied into `usr/bin`. The first is what runs
	/// unless `app_run` says otherwise.
	pub binaries: &'a [&'a Path],
	/// The `.desktop` file. Its `Icon` names the icon without its extension.
	pub desktop_file: &'a Path,
	/// A PNG icon, copied to the root under its own name and as `.DirIcon`.
	pub icon: &'a Path,
	/// The `AppRun` script, for when launching needs more than running the first binary.
	pub app_run: Option<&'a str>,
}

impl AppImage<'_> {
	/// Builds `output` with `appimagetool`, from an `AppDir` beside it. Returns whether it did:
	/// when `appimagetool` is missing or fails, it prints a warning instead, since the `AppImage`
	/// comes on top of the portable archive. Set `APPIMAGETOOL` to use one outside `PATH`.
	pub fn build(&self, output: &Path) -> Result<bool> {
		let app_dir = output.with_extension("AppDir");
		let _ = fs::remove_dir_all(&app_dir);
		let bin_dir = app_dir.join("usr/bin");
		fs::create_dir_all(&bin_dir)?;
		for binary in self.binaries {
			let dest = bin_dir.join(binary.file_name().ok_or("binary path has no file name")?);
			fs::copy(binary, &dest)?;
			#[cfg(unix)]
			make_executable(&dest)?;
		}
		fs::copy(self.desktop_file, app_dir.join(self.desktop_file.file_name().ok_or("desktop file has no name")?))?;
		fs::copy(self.icon, app_dir.join(self.icon.file_name().ok_or("icon has no file name")?))?;
		fs::copy(self.icon, app_dir.join(".DirIcon"))?;
		let default_run = self.binaries.first().and_then(|b| b.file_name()).map(|name| {
			format!(
				r#"#!/bin/sh
HERE="$(dirname "$(readlink -f "${{0}}")")"
exec "${{HERE}}/usr/bin/{}" "$@"
"#,
				name.to_string_lossy()
			)
		});
		let app_run_path = app_dir.join("AppRun");
		fs::write(&app_run_path, self.app_run.map(str::to_string).or(default_run).ok_or("no binaries")?)?;
		#[cfg(unix)]
		make_executable(&app_run_path)?;
		let tool = env::var("APPIMAGETOOL").map_or_else(|_| PathBuf::from("appimagetool"), PathBuf::from);
		// Extracting and running avoids needing FUSE, which CI runners don't have.
		match Command::new(&tool).arg("--appimage-extract-and-run").arg(&app_dir).arg(output).status() {
			Ok(status) if status.success() => Ok(true),
			Ok(status) => {
				println!("Warning: appimagetool exited with {status}, skipping the AppImage.");
				Ok(false)
			}
			Err(err) => {
				println!("Warning: couldn't run appimagetool ({err}), skipping the AppImage. Is it in your PATH?");
				Ok(false)
			}
		}
	}
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
	use std::os::unix::fs::PermissionsExt;
	fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

/// Compiles the Inno Setup script at `iss`, returning whether it made an installer. A missing
/// script or compiler only prints a warning, since the installer comes on top of the portable
/// archive.
pub fn inno_setup(iss: &Path) -> Result<bool> {
	if !iss.exists() {
		println!("Skipping the installer: {} not found.", iss.display());
		return Ok(false);
	}
	match Command::new("ISCC.exe").arg("/Q").arg(iss).status() {
		Ok(status) if status.success() => Ok(true),
		Ok(status) => Err(format!("Inno Setup failed with {status}").into()),
		Err(err) => {
			println!("Skipping the installer: couldn't run ISCC.exe ({err}). Is Inno Setup in your PATH?");
			Ok(false)
		}
	}
}
