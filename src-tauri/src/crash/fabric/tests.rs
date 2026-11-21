use super::*;

const STARTUP_CRASH: &str = include_str!("startup-crash.xml");

#[test]
fn summarizes_a_mod_that_crashed_on_startup_by_its_first_and_innermost_cause() {
    let error = LoaderError::find(STARTUP_CRASH).unwrap();
    assert_eq!(
        error.summary(),
        [
            "net.fabricmc.loader.impl.FormattedException: java.lang.RuntimeException: Could not execute entrypoint stage 'preLaunch' due to errors, provided by 'sodium' at 'net.caffeinemc.mods.sodium.fabric.SodiumPreLaunch'!",
            "org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException: @Mixin target net.minecraft.client.QaMissingTarget was not found qa_failmod.mixins.json:ReadyProbeMixin from mod qa_failmod",
            "mod: qa_failmod",
        ]
    );
    assert_eq!(
        error.fingerprint(),
        [
            "org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException",
            "qa_failmod",
        ]
    );
    let summary = error.summary().join("\n");
    assert!(!summary.contains("secret chat"));
    assert!(!summary.contains("minsu"));
}

#[test]
fn blames_the_entrypoint_mod_when_no_mixin_names_one() {
    let log = STARTUP_CRASH.replace(" from mod qa_failmod", "");
    assert_eq!(LoaderError::find(&log).unwrap().fingerprint()[1], "sodium");
}

#[test]
fn ignores_a_log_whose_last_error_is_not_the_loader_giving_up() {
    let later = format!(
        "{STARTUP_CRASH}  <log4j:Event logger=\"net.minecraft.client.Minecraft\" timestamp=\"1\" level=\"ERROR\" thread=\"Render thread\">\n    <log4j:Message><![CDATA[Failed]]></log4j:Message>\n    <log4j:Throwable><![CDATA[java.lang.IllegalStateException: later\n\tat a.B.c(B.java:1)\n]]></log4j:Throwable>\n  </log4j:Event>\n"
    );
    assert_eq!(LoaderError::find(&later), None);
    let warning = STARTUP_CRASH.replace(
        r#"logger="FabricLoader" timestamp="1795228094010" level="ERROR""#,
        r#"logger="FabricLoader" timestamp="1795228094010" level="WARN""#,
    );
    assert_eq!(LoaderError::find(&warning), None);
}
