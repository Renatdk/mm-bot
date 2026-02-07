use crate::event::EngineEvent;

pub fn consume(events: Vec<EngineEvent>) {
    for e in events {
        match e {
            EngineEvent::Transition { from, cause, to } => {
                println!("Transition: {:?} --({:?})-> {:?}", from, cause, to);
            }
            EngineEvent::PolicyDecision { mode, reason } => {
                println!("Policy: {:?} ({:?})", mode, reason);
            }
            EngineEvent::Log(msg) => {
                println!("Log: {}", msg);
            }
        }
    }
}
