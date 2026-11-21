use std::path::{Path, PathBuf};

use crate::error::LauncherError;
use crate::install::download::io_error;

const BUNDLE: &str = "HyperFarm.app";
const EXECUTABLE: &str = "java";
const ICON: &[u8] = include_bytes!("../../icons/icon.icns");

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>하이퍼팜</string>
  <key>CFBundleDisplayName</key>
  <string>하이퍼팜</string>
  <key>CFBundleIdentifier</key>
  <string>kr.junhyung.hyperfarm.game</string>
  <key>CFBundleExecutable</key>
  <string>{EXECUTABLE}</string>
  <key>CFBundleIconFile</key>
  <string>icon</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
"#
    )
}

// Dock 과 앱 전환기는 실행 파일이 든 번들의 CFBundleName 을 이름으로 쓰고, 번들 밖의 java 는 실행 파일 이름 "java" 로 뜬다.
// -Xdock:name 은 AWT 만 읽어 SDL 로 창을 띄우는 게임에는 먹지 않는다. java 는 libjli 를 링크를 따라간 실제 경로에서 찾으므로
// 번들 안에 링크로 두어도 그대로 뜬다. 런타임 판이 바뀌면 경로도 바뀌므로 실행할 때마다 다시 건다
pub async fn wrap(runtime_root: &Path, java: &Path) -> Result<PathBuf, LauncherError> {
    let contents = runtime_root.join(BUNDLE).join("Contents");
    let executables = contents.join("MacOS");
    let resources = contents.join("Resources");
    for directory in [&executables, &resources] {
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(io_error(directory))?;
    }
    let plist = contents.join("Info.plist");
    tokio::fs::write(&plist, info_plist())
        .await
        .map_err(io_error(&plist))?;
    let icon = resources.join("icon.icns");
    tokio::fs::write(&icon, ICON)
        .await
        .map_err(io_error(&icon))?;

    let executable = executables.join(EXECUTABLE);
    match tokio::fs::remove_file(&executable).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(io_error(&executable)(error));
        }
        _ => {}
    }
    tokio::fs::symlink(java, &executable)
        .await
        .map_err(io_error(&executable))?;
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn follows_the_runtime_when_it_moves() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("java-runtime-delta/bin/java");
        let new = root.path().join("java-runtime-epsilon/bin/java");

        wrap(root.path(), &old).await.unwrap();
        let executable = wrap(root.path(), &new).await.unwrap();

        assert_eq!(tokio::fs::read_link(&executable).await.unwrap(), new);
        assert!(executable.ends_with("HyperFarm.app/Contents/MacOS/java"));
    }
}
