# shipfitter

Build and packaging helpers for Rust desktop apps, so each app's build script and xtask don't carry their own copies. It's used by [Fedra](https://github.com/trypsynth/fedra) and [Paperback](https://github.com/trypsynth/paperback).

## What's in it

- `build`, for build scripts: the commit being built as environment variables, readmes converted with pandoc, `@NAME@` templates such as an Inno Setup script filled in with the version and architecture, and the target directory.
- `macos`: an app's `Info.plist`, and on a Mac, the `.app` bundle, Developer ID signing, and a drag-to-install disk image.
- `package`, with the `package` feature, for an xtask: release builds, zip and tar.gz archives, and compiling an Inno Setup installer.
- `windows`, with the `windows-resources` feature, for build scripts: the application manifest and the version resource Explorer shows.

`build` and `macos` need nothing beyond the standard library.

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

The app can then read `env!("MYAPP_COMMIT_HASH")`, and the installer script can use `@PROJECT_VERSION@`, `@ARCH_SUFFIX@` and `@ARCH_ISS@`.

In an xtask, with the `package` feature:

```rust
use shipfitter::{host_arch_suffix, package::{Zip, cargo_build_release, inno_setup}};

cargo_build_release(&root, &[])?;
let mut zip = Zip::create(&target_dir.join(format!("myapp-{}.zip", host_arch_suffix())))?;
zip.file(&target_dir.join("myapp.exe"), "myapp.exe")?.dir(&root.join("sounds"), "sounds")?;
zip.finish()?;
inno_setup(&target_dir.join("myapp.iss"))?;
```

## License

MIT
