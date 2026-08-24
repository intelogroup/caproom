pub trait AgentAdapter {
    fn name(&self) -> &str;
    fn is_tui(&self) -> bool { false }
    fn preserve_hook(&self) -> Option<&str> { None }
}

pub struct GenericPty;
impl AgentAdapter for GenericPty {
    fn name(&self) -> &str { "generic" }
}

pub struct Claude;
impl AgentAdapter for Claude {
    fn name(&self) -> &str { "claude" }
    fn is_tui(&self) -> bool { true }
}

pub struct Opencode;
impl AgentAdapter for Opencode {
    fn name(&self) -> &str { "opencode" }
    fn is_tui(&self) -> bool { true }
}

pub fn adapter_for(cmd: &str) -> Box<dyn AgentAdapter> {
    match cmd.split('/').last().unwrap_or(cmd) {
        "claude" => Box::new(Claude),
        "opencode" => Box::new(Opencode),
        _ => Box::new(GenericPty),
    }
}
