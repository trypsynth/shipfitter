//! Release builds, archives and installers, for an xtask.

use std::{
	env,
	fs::File,
	io::{self, Write},
	path::Path,
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
