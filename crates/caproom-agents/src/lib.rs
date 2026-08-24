pub trait AgentAdapter {
    fn name(&self) -> &str;
    fn is_tui(&self) -> bool {
        false
    }
    fn preserve_hook(&self) -> Option<&str> {
        None
    }
}

pub struct GenericPty;
impl AgentAdapter for GenericPty {
    fn name(&self) -> &str {
        "generic"
    }
}

pub struct Claude;
impl AgentAdapter for Claude {
    fn name(&self) -> &str {
        "claude"
    }
    fn is_tui(&self) -> bool {
        true
    }
}

pub struct Opencode;
impl AgentAdapter for Opencode {
    fn name(&self) -> &str {
        "opencode"
    }
    fn is_tui(&self) -> bool {
        true
    }
}

pub fn adapter_for(cmd: &str) -> Box<dyn AgentAdapter> {
    match cmd.split('/').next_back().unwrap_or(cmd) {
        "claude" => Box::new(Claude),
        "opencode" => Box::new(Opencode),
        _ => Box::new(GenericPty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_identity_and_tui() {
        assert_eq!(adapter_for("claude").name(), "claude");
        assert_eq!(adapter_for("opencode").name(), "opencode");
        assert_eq!(adapter_for("npm").name(), "generic");
        assert_eq!(adapter_for("/usr/local/bin/claude").name(), "claude");
        assert!(adapter_for("claude").is_tui());
        assert!(adapter_for("opencode").is_tui());
        assert!(!adapter_for("ls").is_tui());
    }
}
