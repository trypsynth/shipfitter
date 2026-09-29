//! Signing release files.
//!
//! A [`CodeSigner`] embeds a signature in an executable or bundle, the kind the operating system
//! checks before running it. [`macos::DeveloperId`](crate::macos::DeveloperId) is one. With the
//! `package` feature, [`Minisign`] writes a detached `.minisig` signature beside a file, the kind
//! an updater checks before installing it.
//!
//! Each signer has a `from_env` constructor for CI, which reads its secrets from environment
//! variables. It returns `None` when none of them are set, so a local build skips that signer,
//! and an error when only some are, so a misnamed secret fails the build instead of quietly
//! shipping unsigned files.

use std::{env, path::Path};

use crate::Result;

/// Signs executables and bundles in place.
pub trait CodeSigner {
	/// Signs the file or bundle at `path`.
	fn sign(&self, path: &Path) -> Result<()>;
}

/// Reads the environment variables in `names`, treating empty ones as unset. Returns `None` when
/// none of them are set, and an error naming the missing ones when only some are.
pub(crate) fn env_group<const N: usize>(names: [&str; N]) -> Result<Option<[String; N]>> {
	let values = names.map(|name| env::var(name).ok().filter(|value| !value.trim().is_empty()));
	if values.iter().all(Option::is_none) {
		return Ok(None);
	}
	let missing: Vec<_> =
		names.iter().zip(&values).filter(|(_, value)| value.is_none()).map(|(name, _)| *name).collect();
	if !missing.is_empty() {
		let set: Vec<_> = names.iter().filter(|name| !missing.contains(name)).copied().collect();
		return Err(format!("{} set without {}", set.join(", "), missing.join(", ")).into());
	}
	Ok(Some(values.map(Option::unwrap_or_default)))
}

#[cfg(feature = "package")]
pub use minisign_signer::Minisign;

#[cfg(feature = "package")]
mod minisign_signer {
	use std::{
		fs::{self, File},
		io::BufReader,
		path::{Path, PathBuf},
		time::{SystemTime, UNIX_EPOCH},
	};

	use minisign::{SecretKey, SecretKeyBox};

	use crate::Result;

	/// Writes minisign signatures, which updaters such as ship-shape check downloads against.
	pub struct Minisign {
		key: SecretKey,
	}

	impl Minisign {
		/// Loads a secret key in the form `minisign -G` writes it, decrypting it with `password`.
		/// Pass `None` for a key made without a password.
		pub fn new(key: &str, password: Option<&str>) -> Result<Self> {
			let key =
				SecretKeyBox::from_string(key)?.into_secret_key(Some(password.unwrap_or_default().to_string()))?;
			Ok(Self { key })
		}

		/// The key in `MINISIGN_KEY` and its password in `MINISIGN_PASSWORD`, or `None` when
		/// `MINISIGN_KEY` isn't set.
		pub fn from_env() -> Result<Option<Self>> {
			let Some([key]) = super::env_group(["MINISIGN_KEY"])? else { return Ok(None) };
			let password = std::env::var("MINISIGN_PASSWORD").ok();
			Self::new(&key, password.as_deref()).map(Some).map_err(|e| format!("MINISIGN_KEY: {e}").into())
		}

		/// Signs `path`, writing the signature to `path` with `.minisig` appended, and returns
		/// where it went. The trusted comment is the one the minisign tool writes.
		pub fn sign(&self, path: &Path) -> Result<PathBuf> {
			let name = path.file_name().ok_or("path has no file name")?.to_string_lossy();
			let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
			let trusted = format!("timestamp:{timestamp}\tfile:{name}\thashed");
			let reader = BufReader::new(File::open(path)?);
			let signature = minisign::sign(None, &self.key, reader, Some(&trusted), None)?;
			let mut output = path.as_os_str().to_owned();
			output.push(".minisig");
			let output = PathBuf::from(output);
			fs::write(&output, signature.into_string())?;
			Ok(output)
		}
	}

	#[cfg(test)]
	mod tests {
		use minisign::KeyPair;

		use super::*;

		#[test]
		fn signatures_verify_with_minisign_verify() {
			let dir = std::env::temp_dir().join(format!("shipfitter-minisign-{}", std::process::id()));
			fs::create_dir_all(&dir).unwrap();
			let file = dir.join("app-x64.zip");
			fs::write(&file, b"release contents").unwrap();
			let pair = KeyPair::generate_encrypted_keypair(Some("hunter2".to_string())).unwrap();
			let secret = pair.sk.to_box(None).unwrap().into_string();
			let public = pair.pk.to_box().unwrap().into_string();

			let signer = Minisign::new(&secret, Some("hunter2")).unwrap();
			let signature_path = signer.sign(&file).unwrap();
			assert_eq!(signature_path, dir.join("app-x64.zip.minisig"));

			let public = minisign_verify::PublicKey::from_base64(public.lines().nth(1).unwrap()).unwrap();
			let signature = minisign_verify::Signature::decode(&fs::read_to_string(&signature_path).unwrap()).unwrap();
			assert!(signature.trusted_comment().ends_with("\tfile:app-x64.zip\thashed"));
			public.verify(&fs::read(&file).unwrap(), &signature, false).unwrap();
			assert!(public.verify(b"tampered", &signature, false).is_err());
			fs::remove_dir_all(&dir).unwrap();
		}

		#[test]
		fn wrong_password_fails() {
			let pair = KeyPair::generate_encrypted_keypair(Some("right".to_string())).unwrap();
			let secret = pair.sk.to_box(None).unwrap().into_string();
			assert!(Minisign::new(&secret, Some("wrong")).is_err());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn env_group_is_all_or_nothing() {
		let [a, b] = ["SHIPFITTER_TEST_GROUP_A", "SHIPFITTER_TEST_GROUP_B"];
		// SAFETY: no other test reads or writes these variables.
		unsafe {
			env::remove_var(a);
			env::remove_var(b);
		}
		assert!(env_group([a, b]).unwrap().is_none());
		unsafe { env::set_var(a, "1") };
		let err = env_group([a, b]).unwrap_err().to_string();
		assert_eq!(err, format!("{a} set without {b}"));
		unsafe { env::set_var(b, " ") };
		assert!(env_group([a, b]).is_err(), "blank counts as unset");
		unsafe { env::set_var(b, "2") };
		assert_eq!(env_group([a, b]).unwrap(), Some(["1".to_string(), "2".to_string()]));
		unsafe {
			env::remove_var(a);
			env::remove_var(b);
		}
	}
}
