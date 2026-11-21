use regex_lite::Regex;

// 게임 출력은 Minecraft 의 log4j 설정이 정한 XML 이다. loader 가 시작 단계의 치명적 오류를 남기는 것은 이 logger 의 ERROR
// 이벤트뿐이고, 모드의 mixin 경고는 FabricLoader/Mixin 으로 따로 나온다
const LOADER_ERROR: &str = r#"logger="FabricLoader" "#;
const THROWABLE_START: &str = "<log4j:Throwable><![CDATA[";
const THROWABLE_END: &str = "]]></log4j:Throwable>";
const EVENT_START: &str = "<log4j:Event ";
const CAUSED_BY: &str = "Caused by: ";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct LoaderError {
    first: String,
    innermost: Option<String>,
    mod_id: Option<String>,
}

impl LoaderError {
    // 게임 로그에는 채팅이 섞여 있다. 그 이벤트의 예외 줄 말고는 아무것도 꺼내지 않는다
    pub(super) fn find(log: &str) -> Option<Self> {
        let start = log.rfind(THROWABLE_START)?;
        let event = &log[log[..start].rfind(EVENT_START)?..start];
        if !event.contains(LOADER_ERROR) || !event.contains(r#"level="ERROR""#) {
            return None;
        }
        let body = &log[start + THROWABLE_START.len()..];
        let throwable = body.find(THROWABLE_END).map_or(body, |end| &body[..end]);
        let first = throwable.lines().next()?.trim().to_string();
        let innermost = throwable
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix(CAUSED_BY))
            .map(|line| line.trim().to_string());
        let mod_id = culprit(innermost.as_deref().unwrap_or(&first), throwable);
        Some(Self {
            first,
            innermost,
            mod_id,
        })
    }

    pub(super) fn summary(&self) -> Vec<String> {
        std::iter::once(self.first.clone())
            .chain(self.innermost.clone())
            .chain(self.mod_id.as_ref().map(|id| format!("mod: {id}")))
            .collect()
    }

    pub(super) fn fingerprint(&self) -> Vec<String> {
        let innermost = self.innermost.as_deref().unwrap_or(&self.first);
        let class = innermost.split(':').next().unwrap_or(innermost).trim();
        std::iter::once(class.to_string())
            .chain(self.mod_id.clone())
            .collect()
    }
}

// Mixin 은 실패한 mixin 의 모드를 from mod 로 적고, loader 는 그 mixin 이 걸린 entrypoint 의 모드를 provided by 로 적는다.
// 예에서는 sodium 의 preLaunch 가 qa 모드의 mixin 때문에 실패하므로, 탓할 모드는 from mod 쪽이다
fn culprit(innermost: &str, throwable: &str) -> Option<String> {
    let from_mod = Regex::new(r"from mod ([a-z][a-z0-9_-]*)").expect("the mod pattern is valid");
    let provided_by =
        Regex::new(r"provided by '([a-z][a-z0-9_-]*)'").expect("the mod pattern is valid");
    let causes = || {
        throwable
            .lines()
            .filter(|line| !line.starts_with(char::is_whitespace))
            .rev()
    };
    std::iter::once(innermost)
        .chain(causes())
        .find_map(|line| from_mod.captures(line))
        .or_else(|| causes().find_map(|line| provided_by.captures(line)))
        .map(|captures| captures[1].to_string())
}

#[cfg(test)]
mod tests;
