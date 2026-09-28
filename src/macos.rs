//! macOS app bundles and disk images.
//!
//! [`MacApp::info_plist`] works anywhere, so a build script can lay out the bundle. The rest runs
//! Apple's command line tools and only works on a Mac.

use std::{
	env, fs,
	path::{Path, PathBuf},
	process::Command,
};

use crate::Result;

/// What goes in an app's `Info.plist`.
#[derive(Debug, Clone, Default)]
pub struct MacApp<'a> {
	/// The name shown in Finder and the menu bar, such as `Fedra`. The bundle is `{name}.app`.
	pub name: &'a str,
	/// The reverse DNS bundle identifier, such as `com.trypsynth.fedra`.
	pub identifier: &'a str,
	/// The executable's file name inside `Contents/MacOS`.
	pub executable: &'a str,
	pub version: &'a str,
	/// The `.icns` file in `Contents/Resources`, without its extension.
	pub icon: Option<&'a str>,
	/// Raw keys and values added to the end of the top level dictionary, such as
	/// `CFBundleDocumentTypes`.
	pub extra: &'a str,
}

fn run(command: &mut Command, what: &str) -> Result<()> {
	if !command.status()?.success() {
		return Err(format!("{what} failed").into());
	}
	Ok(())
}

impl MacApp<'_> {
	#[must_use]
	pub fn info_plist(&self) -> String {
		let icon = self.icon.map(|icon| format!("\t<key>CFBundleIconFile</key>\n\t<string>{icon}</string>\n"));
		format!(
			r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundleIdentifier</key>
	<string>{identifier}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleExecutable</key>
	<string>{executable}</string>
{icon}	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>NSHighResolutionCapable</key>
	<true/>
{extra}</dict>
</plist>
"#,
			name = self.name,
			identifier = self.identifier,
			version = self.version,
			executable = self.executable,
			icon = icon.unwrap_or_default(),
			extra = self.extra,
		)
	}

	/// Builds `{name}.app` in `dir` from scratch, with `binary` as its executable. Each of
	/// `macos` is copied into `Contents/MacOS` beside it, for libraries it loads, and each of
	/// `resources`, file or folder, into `Contents/Resources`.
	pub fn bundle(&self, dir: &Path, binary: &Path, macos: &[&Path], resources: &[&Path]) -> Result<PathBuf> {
		let bundle = dir.join(format!("{}.app", self.name));
		let _ = fs::remove_dir_all(&bundle);
		let macos_dir = bundle.join("Contents/MacOS");
		let resources_dir = bundle.join("Contents/Resources");
		fs::create_dir_all(&macos_dir)?;
		fs::create_dir_all(&resources_dir)?;
		fs::write(bundle.join("Contents/Info.plist"), self.info_plist())?;
		run(Command::new("ditto").arg(binary).arg(macos_dir.join(self.executable)), "Copying the executable")?;
		for (paths, dest) in [(macos, &macos_dir), (resources, &resources_dir)] {
			for path in paths {
				let name = path.file_name().ok_or("path has no file name")?;
				run(Command::new("ditto").arg(path).arg(dest.join(name)), "Copying into the bundle")?;
			}
		}
		Ok(bundle)
	}
}

/// Signs `bundle` for distribution, so it can be notarized.
///
/// The Developer ID identity is named by the `MACOS_SIGN_IDENTITY` environment variable. Each of `nested`, paths
/// relative to the bundle such as `Contents/MacOS/libpdfium.dylib`, is signed first, then the
/// executables in `Contents/MacOS`, then the bundle, innermost first as Apple recommends over
/// `--deep`.
///
/// Returns whether it signed anything: without the variable, the bundle keeps the ad hoc
/// signature the linker gave it, which is enough to run it locally.
pub fn sign(bundle: &Path, nested: &[&str]) -> Result<bool> {
	let Ok(identity) = env::var("MACOS_SIGN_IDENTITY") else {
		println!("MACOS_SIGN_IDENTITY not set, skipping code signing.");
		return Ok(false);
	};
	let mut paths: Vec<PathBuf> = nested.iter().map(|path| bundle.join(path)).collect();
	for entry in fs::read_dir(bundle.join("Contents/MacOS"))? {
		let path = entry?.path();
		if !paths.contains(&path) {
			paths.push(path);
		}
	}
	paths.push(bundle.to_path_buf());
	for path in paths {
		run(
			Command::new("codesign")
				.args(["--force", "--timestamp", "--options", "runtime", "--sign", &identity])
				.arg(&path),
			&format!("Signing {}", path.display()),
		)?;
	}
	Ok(true)
}

/// Creates a compressed disk image at `output` holding `bundle` beside a link to Applications,
/// so installing is the usual drag. Its volume is named after the bundle.
pub fn dmg(bundle: &Path, output: &Path) -> Result<()> {
	let name = bundle.file_name().ok_or("bundle path has no file name")?;
	let volume = Path::new(name).file_stem().ok_or("bundle path has no file name")?;
	let staging = output.with_extension("dmg-staging");
	let _ = fs::remove_dir_all(&staging);
	fs::create_dir_all(&staging)?;
	run(Command::new("ditto").arg(bundle).arg(staging.join(name)), "Copying the bundle")?;
	run(Command::new("ln").arg("-s").arg("/Applications").arg(staging.join("Applications")), "Linking Applications")?;
	run(
		Command::new("hdiutil")
			.args(["create", "-format", "UDZO", "-ov", "-volname"])
			.arg(volume)
			.arg("-srcfolder")
			.arg(&staging)
			.arg(output),
		"hdiutil create",
	)?;
	let _ = fs::remove_dir_all(&staging);
	Ok(())
}
