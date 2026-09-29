//! macOS app bundles, signing, notarization, and disk images.
//!
//! [`MacApp::info_plist`] works anywhere, so a build script can lay out the bundle. The rest runs
//! Apple's command line tools and only works on a Mac.
//!
//! A signed and notarized release goes through [`Keychain::import_from_env`], [`sign_bundle`],
//! [`dmg`], and [`Notary::notarize`], in that order.

use std::{
	env,
	fmt::Write as _,
	fs,
	io::Read,
	path::{Path, PathBuf},
	process::{Command, Output},
};

use crate::{
	Result,
	sign::{CodeSigner, env_group},
};

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

/// Signs with a Developer ID Application certificate in a keychain, with the hardened runtime and
/// the secure timestamp that notarization requires.
#[derive(Debug, Clone)]
pub struct DeveloperId {
	/// The identity's name, such as `Developer ID Application: Jane Doe (TEAMID)`, or its hash.
	pub identity: String,
}

impl DeveloperId {
	/// The identity named by `MACOS_SIGN_IDENTITY`, or `None` when it isn't set.
	#[must_use]
	pub fn from_env() -> Option<Self> {
		env::var("MACOS_SIGN_IDENTITY").ok().filter(|identity| !identity.is_empty()).map(|identity| Self { identity })
	}
}

impl CodeSigner for DeveloperId {
	fn sign(&self, path: &Path) -> Result<()> {
		run(
			Command::new("codesign")
				.args(["--force", "--timestamp", "--options", "runtime", "--sign", &self.identity])
				.arg(path),
			&format!("Signing {}", path.display()),
		)
	}
}

/// Signs `bundle` and everything in it with `signer`, so it can be notarized.
///
/// Each of `nested`, paths relative to the bundle such as `Contents/MacOS/libpdfium.dylib`, is
/// signed first, then the executables in `Contents/MacOS`, then the bundle: innermost first, as
/// Apple recommends over `--deep`.
pub fn sign_bundle(bundle: &Path, nested: &[&str], signer: &dyn CodeSigner) -> Result<()> {
	let mut paths: Vec<PathBuf> = nested.iter().map(|path| bundle.join(path)).collect();
	for entry in fs::read_dir(bundle.join("Contents/MacOS"))? {
		let path = entry?.path();
		if !paths.contains(&path) {
			paths.push(path);
		}
	}
	paths.push(bundle.to_path_buf());
	paths.iter().try_for_each(|path| signer.sign(path))
}

/// [`sign_bundle`] with the [`DeveloperId::from_env`] identity.
///
/// Returns whether it signed anything. Without `MACOS_SIGN_IDENTITY`, the bundle keeps the ad hoc
/// signature the linker gave it, which is enough to run it locally.
pub fn sign(bundle: &Path, nested: &[&str]) -> Result<bool> {
	let Some(signer) = DeveloperId::from_env() else {
		println!("MACOS_SIGN_IDENTITY not set, skipping code signing.");
		return Ok(false);
	};
	sign_bundle(bundle, nested, &signer)?;
	Ok(true)
}

/// A temporary keychain holding a signing certificate, for CI runners that don't have one.
///
/// Dropping it deletes the keychain and restores the keychain search list.
#[derive(Debug)]
pub struct Keychain {
	path: PathBuf,
	search_list: Vec<String>,
	/// The Developer ID Application identity from the certificate.
	pub identity: String,
}

impl Keychain {
	/// Imports the base64-encoded `.p12` file in `MACOS_CERTIFICATE_P12_BASE64`, whose password
	/// is in `MACOS_CERTIFICATE_PASSWORD`. Returns `None` when neither is set.
	pub fn import_from_env() -> Result<Option<Self>> {
		let Some([p12, password]) = env_group(["MACOS_CERTIFICATE_P12_BASE64", "MACOS_CERTIFICATE_PASSWORD"])? else {
			return Ok(None);
		};
		let p12 = base64_decode(&p12).ok_or("MACOS_CERTIFICATE_P12_BASE64 isn't valid base64")?;
		Self::import(&p12, &password).map(Some)
	}

	/// Imports the certificate and private key in the `.p12` data `p12` into a new keychain, and
	/// finds the Developer ID Application identity in it.
	pub fn import(p12: &[u8], password: &str) -> Result<Self> {
		let dir = env::temp_dir();
		let id = std::process::id();
		let path = dir.join(format!("shipfitter-signing-{id}.keychain-db"));
		let keychain = path.to_str().ok_or("the temporary directory isn't valid UTF-8")?.to_string();
		// Nothing needs the keychain's password after this, so it's random rather than a secret.
		let keychain_password = random_hex(16)?;
		let search_list = keychain_search_list()?;
		let _ = security(&["delete-keychain", &keychain]);
		security(&["create-keychain", "-p", &keychain_password, &keychain])?;
		// From here on, dropping this deletes the keychain, even if a later step fails.
		let mut this = Self { path, search_list, identity: String::new() };
		security(&["set-keychain-settings", "-lut", "21600", &keychain])?;
		security(&["unlock-keychain", "-p", &keychain_password, &keychain])?;
		let cert = dir.join(format!("shipfitter-signing-{id}.p12"));
		fs::write(&cert, p12)?;
		let cert_path = cert.to_string_lossy().into_owned();
		let imported =
			security(&["import", &cert_path, "-P", password, "-A", "-t", "cert", "-f", "pkcs12", "-k", &keychain]);
		let _ = fs::remove_file(&cert);
		imported?;
		let mut search = vec!["list-keychains", "-d", "user", "-s", keychain.as_str()];
		search.extend(this.search_list.iter().map(String::as_str));
		security(&search)?;
		security(&[
			"set-key-partition-list",
			"-S",
			"apple-tool:,apple:,codesign:",
			"-s",
			"-k",
			&keychain_password,
			&keychain,
		])?;
		let identities = security(&["find-identity", "-v", "-p", "codesigning", &keychain])?;
		this.identity = developer_id_identity(&identities)
			.ok_or("the certificate has no Developer ID Application identity")?
			.to_string();
		Ok(this)
	}

	/// A signer that uses this keychain's identity.
	#[must_use]
	pub fn signer(&self) -> DeveloperId {
		DeveloperId { identity: self.identity.clone() }
	}
}

impl Drop for Keychain {
	fn drop(&mut self) {
		let mut restore = vec!["list-keychains", "-d", "user", "-s"];
		restore.extend(self.search_list.iter().map(String::as_str));
		let _ = security(&restore);
		let _ = security(&["delete-keychain", &self.path.to_string_lossy()]);
	}
}

/// Runs `security`, returning its standard output, or an error with its standard error.
fn security(args: &[&str]) -> Result<String> {
	let output = Command::new("security").args(args).output()?;
	checked(&output, &format!("security {}", args[0]))?;
	Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn checked(output: &Output, what: &str) -> Result<()> {
	if output.status.success() {
		return Ok(());
	}
	Err(format!("{what} failed: {}", String::from_utf8_lossy(&output.stderr).trim()).into())
}

fn keychain_search_list() -> Result<Vec<String>> {
	Ok(security(&["list-keychains", "-d", "user"])?
		.lines()
		.map(|line| line.trim().trim_matches('"').to_string())
		.filter(|line| !line.is_empty())
		.collect())
}

/// The quoted name on the first `Developer ID Application` line of `security find-identity`.
fn developer_id_identity(find_identity: &str) -> Option<&str> {
	let line = find_identity.lines().find(|line| line.contains("\"Developer ID Application"))?;
	let start = line.find('"')? + 1;
	let end = line.rfind('"')?;
	(start < end).then(|| &line[start..end])
}

fn random_hex(bytes: usize) -> Result<String> {
	let mut buffer = vec![0; bytes];
	fs::File::open("/dev/urandom")?.read_exact(&mut buffer)?;
	Ok(buffer.iter().fold(String::new(), |mut hex, byte| {
		let _ = write!(hex, "{byte:02x}");
		hex
	}))
}

/// Apple's notary service, authenticated with an App Store Connect API key.
#[derive(Debug, Clone)]
pub struct Notary {
	/// The contents of the `.p8` key file.
	pub key: Vec<u8>,
	pub key_id: String,
	pub issuer_id: String,
}

impl Notary {
	/// The base64-encoded `.p8` key in `APPSTORE_API_KEY_BASE64`, its ID in
	/// `APPSTORE_API_KEY_ID`, and the issuer ID in `APPSTORE_API_ISSUER_ID`. Returns `None` when
	/// none of them are set.
	pub fn from_env() -> Result<Option<Self>> {
		let Some([key, key_id, issuer_id]) =
			env_group(["APPSTORE_API_KEY_BASE64", "APPSTORE_API_KEY_ID", "APPSTORE_API_ISSUER_ID"])?
		else {
			return Ok(None);
		};
		let key = base64_decode(&key).ok_or("APPSTORE_API_KEY_BASE64 isn't valid base64")?;
		// IDs pasted into a secret often pick up a trailing newline.
		let [key_id, issuer_id] = [key_id, issuer_id].map(|id| id.split_whitespace().collect());
		Ok(Some(Self { key, key_id, issuer_id }))
	}

	/// Notarizes `path`, a signed disk image, zip file, or installer package, and staples the
	/// ticket to it so Gatekeeper can check it offline.
	///
	/// Waits for the notary service to finish. If it rejects the file, prints the notary log,
	/// which says what to fix, and returns an error.
	pub fn notarize(&self, path: &Path) -> Result<()> {
		let key_path = env::temp_dir().join(format!("shipfitter-notary-{}.p8", std::process::id()));
		fs::write(&key_path, &self.key)?;
		let result = self.submit(path, &key_path);
		let _ = fs::remove_file(&key_path);
		result?;
		run(Command::new("xcrun").args(["stapler", "staple"]).arg(path), "Stapling")
	}

	fn submit(&self, path: &Path, key_path: &Path) -> Result<()> {
		let notarytool = |args: &[&str]| {
			let mut command = Command::new("xcrun");
			command.arg("notarytool").args(args);
			command.arg("--key").arg(key_path).args(["--key-id", &self.key_id, "--issuer", &self.issuer_id]);
			command
		};
		println!("Notarizing {}...", path.display());
		let output = notarytool(&["submit", "--wait", "--output-format", "json"]).arg(path).output()?;
		let json = String::from_utf8_lossy(&output.stdout);
		let status = json_string(&json, "status");
		if output.status.success() && status == Some("Accepted") {
			return Ok(());
		}
		match json_string(&json, "id") {
			Some(id) => {
				let _ = notarytool(&["log", id]).status();
			}
			None => eprintln!("{}", String::from_utf8_lossy(&output.stderr).trim()),
		}
		Err(format!("notarizing {} ended with status {}", path.display(), status.unwrap_or("unknown")).into())
	}
}

/// The string value of `key` in the flat JSON object that notarytool prints.
fn json_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
	let after_key = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
	let value = after_key.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
	value.find('"').map(|end| &value[..end])
}

/// Decodes standard base64, ignoring whitespace such as the line breaks `base64` adds.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
	let mut output = Vec::with_capacity(text.len() * 3 / 4);
	let mut buffer = 0u32;
	let mut bits = 0;
	for byte in text.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
		let value = match byte {
			b'A'..=b'Z' => byte - b'A',
			b'a'..=b'z' => byte - b'a' + 26,
			b'0'..=b'9' => byte - b'0' + 52,
			b'+' => 62,
			b'/' => 63,
			b'=' => break,
			_ => return None,
		};
		buffer = (buffer << 6) | u32::from(value);
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			output.push((buffer >> bits).to_le_bytes()[0]);
		}
	}
	Some(output)
}

/// Copies `bundle` to a compressed disk image at `output`, beside a link to Applications, so
/// installing is the usual drag. The volume is named after the bundle.
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn decodes_base64() {
		assert_eq!(base64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
		assert_eq!(base64_decode("aGVs\nbG8=\n").unwrap(), b"hello");
		assert_eq!(base64_decode("").unwrap(), b"");
		assert!(base64_decode("aGV*").is_none());
	}

	#[test]
	fn reads_notarytool_json() {
		let json =
			r#"{"id":"2efe2717-52ef-43a5-96dc-0797e4ca1041","message":"Processing complete","status" : "Invalid"}"#;
		assert_eq!(json_string(json, "id"), Some("2efe2717-52ef-43a5-96dc-0797e4ca1041"));
		assert_eq!(json_string(json, "status"), Some("Invalid"));
		assert_eq!(json_string(json, "missing"), None);
	}

	#[test]
	fn finds_the_developer_id_identity() {
		let output = "  1) 0123ABCD \"Apple Development: Jane Doe (AAAA)\"\n  2) 4567EF01 \"Developer ID Application: Jane Doe (TEAM123)\"\n     2 valid identities found\n";
		assert_eq!(developer_id_identity(output), Some("Developer ID Application: Jane Doe (TEAM123)"));
		assert_eq!(developer_id_identity("     0 valid identities found\n"), None);
	}
}
