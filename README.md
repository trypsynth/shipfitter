# shipfitter

Build and packaging helpers for Rust desktop apps. [Fedra](https://github.com/trypsynth/fedra) and [Paperback](https://github.com/trypsynth/paperback) use it.

## Modules

- `build`: helpers for build scripts. Embeds the current commit as environment variables, fills in `@NAME@` templates such as an Inno Setup script, and finds the target directory.
- `docs`: converts Markdown readmes to standalone HTML pages with a table of contents, and finds each translated `readme-<lang>.md` in a folder. Requires the `docs` feature.
- `macos`: writes an app's `Info.plist`. On macOS, it also builds the `.app` bundle, imports a signing certificate into a temporary keychain, signs the bundle with a Developer ID, creates a disk image, and notarizes it.
- `package`: helpers for an xtask. Runs release builds, reads a workspace crate's version, writes zip and tar.gz archives, and builds AppImages and Inno Setup installers. Requires the `package` feature.
- `sign`: the `CodeSigner` trait that platform code signers implement. With the `package` feature, also writes minisign signatures for updaters to verify.
- `tools`: finds Inno Setup and appimagetool on `PATH`. If a tool isn't installed, downloads a pinned release to a per-user cache and verifies its SHA-256 checksum. Requires the `package` feature.
- `windows`: embeds the application manifest and the version resource that File Explorer shows. Requires the `windows-resources` feature.

The `build` and `macos` modules depend only on the standard library.

## Usage

In a build script:

```rust
use shipfitter::build::{configure_file, embed_commit_info, target_profile_dir};

fn main() {
	embed_commit_info("MYAPP");
	if let Some(target_dir) = target_profile_dir() {
		configure_file("myapp.iss.in".as_ref(), &target_dir.join("myapp.iss"), &[]).unwrap();
	}
}
```

The app can then read `env!("MYAPP_COMMIT_HASH")`, and the installer script can use `@PROJECT_VERSION@`, `@ARCH_SUFFIX@`, and `@ARCH_ISS@`.

To convert `doc/readme.md` and its translations to HTML next to the executable, enable the `docs` feature:

```rust
use shipfitter::docs::{Page, bcp47, convert, readmes};

println!("cargo:rerun-if-changed=doc");
for readme in readmes("doc".as_ref()).unwrap() {
	let lang = bcp47(&readme.code);
	convert(&readme.path, &target_dir.join(readme.html_name()), &Page { lang: &lang, ..Page::default() }).unwrap();
}
```

In an xtask, enable the `package` feature:

```rust
use shipfitter::{host_arch_suffix, package::{Zip, cargo_build_release, inno_setup}};

cargo_build_release(&root, &[])?;
let mut zip = Zip::create(&target_dir.join(format!("myapp-{}.zip", host_arch_suffix())))?;
zip.file(&target_dir.join("myapp.exe"), "myapp.exe")?.dir(&root.join("sounds"), "sounds")?;
zip.finish()?;
inno_setup(&target_dir.join("myapp.iss"))?;
```

## Signing in CI

Each signer reads its secrets from environment variables. If none of a signer's variables are set, the signer is skipped, so local builds work without secrets. If only some are set, the build fails.

| Signer | Variables |
| --- | --- |
| `sign::Minisign` | `MINISIGN_KEY`, and `MINISIGN_PASSWORD` if the key has one |
| `macos::Keychain` | `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD` |
| `macos::DeveloperId` | `MACOS_SIGN_IDENTITY`, for a certificate that's already in a keychain |
| `macos::Notary` | `APPSTORE_API_KEY_BASE64`, `APPSTORE_API_KEY_ID`, `APPSTORE_API_ISSUER_ID` |

## License

MIT
