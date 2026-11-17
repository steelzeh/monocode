//! Sandboxed interactive visual programs and the native trees they produce.
//!
//! Programs are JavaScript function bodies that return a synchronous render
//! function. The only application capability is the frozen `visual` component
//! API; there is no filesystem, network, shell, DOM, or module loader.
mod model;
pub use model::*;

use rquickjs::{Context, Function, Object, Persistent, Runtime};
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

/// Conservative per-interpreter bound for transcript cache accounting.
pub const MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const TREE_BYTES: usize = 1024 * 1024;
const CALL_DEADLINE: Duration = Duration::from_millis(60);
/// Tweaks are measured by the clock on a machine that may be busy compiling,
/// so a normal script must not run out of time. A runaway one is still stopped.
const TWEAK_CALL_DEADLINE: Duration = Duration::from_millis(250);
const MAX_TWEAK_CALLS: usize = 16;

/// An isolated program. Keep it alive to preserve state between native events.
/// Field order ensures all persistent JS values are dropped before their VM.
pub struct VisualRuntime {
    controller: Persistent<Object<'static>>,
    context: Context,
    _runtime: Runtime,
    deadline: Rc<Cell<Instant>>,
    budget: Duration,
    interrupted: Rc<Cell<bool>>,
    stopped: bool,
    current: Option<Node>,
}

impl VisualRuntime {
    pub fn new(source: &str) -> Result<Self, String> {
        Self::start(source, None, false)
    }

    /// A tweak's panel: the same sandbox, plus a `tweak` object with the JSON
    /// `context` and the actions the app reads through [`Self::take_actions`].
    pub fn with_host(source: &str, context: &str) -> Result<Self, String> {
        Self::start(source, Some(context), false)
    }

    /// A tweak's script: the same sandbox with no view. It registers handlers
    /// on `tweak` and runs when the user clicks one of its menu items.
    pub fn tweak_script(source: &str) -> Result<Self, String> {
        Self::start(source, Some("{}"), true)
    }

    fn start(source: &str, host: Option<&str>, script: bool) -> Result<Self, String> {
        validate_source(source)?;
        let runtime = Runtime::new().map_err(|e| format!("Could not start visual runtime: {e}"))?;
        runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
        runtime.set_max_stack_size(256 * 1024);
        let budget = if host.is_some() {
            TWEAK_CALL_DEADLINE
        } else {
            CALL_DEADLINE
        };
        let deadline = Rc::new(Cell::new(Instant::now() + budget));
        let interrupted = Rc::new(Cell::new(false));
        let interrupt_deadline = deadline.clone();
        let interrupt_flag = interrupted.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let expired = Instant::now() >= interrupt_deadline.get();
            if expired {
                interrupt_flag.set(true);
            }
            expired
        })));
        let context = Context::full(&runtime)
            .map_err(|e| format!("Could not initialize visual context: {e}"))?;
        let controller = context.with(|ctx| {
            let result = (|| -> rquickjs::Result<Persistent<Object<'static>>> {
                let controller: Object = ctx.eval(include_str!("visual/sdk.js"))?;
                let init: Function = controller.get("init")?;
                init.call::<_, ()>((source, host, script))?;
                Ok(Persistent::save(&ctx, controller))
            })();
            result.map_err(|e| execution_error(&ctx, e, &interrupted, budget))
        })?;
        Ok(Self {
            controller,
            context,
            _runtime: runtime,
            deadline,
            budget,
            interrupted,
            stopped: false,
            current: None,
        })
    }

    pub fn render(&mut self) -> Result<Node, String> {
        self.call("render", Call::Plain)
    }

    /// After an error or timeout the program must be started again.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Gives a tweak's panel fresh context. Its own state stays.
    pub fn set_context(&mut self, context: &str) -> Result<Node, String> {
        self.call("setContext", Call::Context(context))
    }

    /// Runs a script's handler for one of its menu items, with the chat as JSON.
    pub fn run_menu(&mut self, entry: &str, chat: &str) -> Result<(), String> {
        self.call_script("runMenu", entry, chat)
    }

    /// Hands the answer for an earlier request to the script's callback.
    pub fn deliver(&mut self, request: u64, answer: &str) -> Result<(), String> {
        self.call_script("deliver", &request.to_string(), answer)
    }

    fn call_script(&mut self, method: &str, first: &str, second: &str) -> Result<(), String> {
        if self.stopped {
            return Err("This tweak stopped after an error".into());
        }
        self.deadline.set(Instant::now() + self.budget);
        self.interrupted.set(false);
        let result = self.context.with(|ctx| {
            let result = (|| -> rquickjs::Result<()> {
                let controller = self.controller.clone().restore(&ctx)?;
                let function: Function = controller.get(method)?;
                function.call((first, second))
            })();
            result.map_err(|e| execution_error(&ctx, e, &self.interrupted, self.budget))
        });
        if result.is_err() {
            self.stopped = true;
        }
        result
    }

    /// Runs a panel's refresh handlers and draws it again.
    pub fn refresh_panel(&mut self) -> Result<Node, String> {
        self.call("refresh", Call::Plain)
    }

    /// Gives a panel the answer to an earlier call, and draws it again.
    pub fn deliver_to_panel(&mut self, request: u64, answer: &str) -> Result<Node, String> {
        self.call("deliver", Call::Event(&request.to_string(), answer))
    }

    /// Ends a tweak the app has refused to keep running.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Drains the app calls a tweak made. Rust parses them again, so a script
    /// cannot invent a request the app does not understand.
    pub fn take_calls(&mut self) -> Result<Vec<crate::tweaks::AppCall>, String> {
        if self.stopped {
            return Ok(Vec::new());
        }
        self.deadline.set(Instant::now() + self.budget);
        self.interrupted.set(false);
        let result = self
            .context
            .with(|ctx| {
                let result = (|| -> rquickjs::Result<String> {
                    let controller = self.controller.clone().restore(&ctx)?;
                    let function: Function = controller.get("takeCalls")?;
                    function.call(())
                })();
                result.map_err(|e| execution_error(&ctx, e, &self.interrupted, self.budget))
            })
            .and_then(|json| {
                serde_json::from_str::<Vec<crate::tweaks::AppCall>>(&json)
                    .map_err(|e| format!("Invalid tweak call: {e}"))
            })
            .and_then(|calls| {
                if calls.len() > MAX_TWEAK_CALLS {
                    Err("Too many tweak calls at once".to_owned())
                } else {
                    Ok(calls)
                }
            });
        if result.is_err() {
            self.stopped = true;
        }
        result
    }

    /// Dispatch only the current render's handler. Values are checked against the
    /// original control before script code sees them (slider bounds, tab index,
    /// boolean toggle). Stale callbacks cannot mutate a replacement render.
    pub fn dispatch(&mut self, handler: &str, value: serde_json::Value) -> Result<Node, String> {
        if handler.len() > 64 {
            return Err("Invalid visual event handler".into());
        }
        self.current
            .as_ref()
            .and_then(|node| validate_event(node, handler, &value))
            .unwrap_or_else(|| Err("This visual control is stale; use the current view".into()))?;
        let payload = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        if payload.len() > 32 * 1024 {
            return Err("Visual event payload is too large".into());
        }
        self.call("dispatch", Call::Event(handler, &payload))
    }

    fn call(&mut self, method: &str, input: Call<'_>) -> Result<Node, String> {
        if self.stopped {
            return Err("Visual stopped after an execution error; reload it to continue".into());
        }
        self.deadline.set(Instant::now() + self.budget);
        self.interrupted.set(false);
        let result = self.context.with(|ctx| {
            let result = (|| -> rquickjs::Result<String> {
                let controller = self.controller.clone().restore(&ctx)?;
                let function: Function = controller.get(method)?;
                match input {
                    Call::Event(handler, payload) => function.call((handler, payload)),
                    Call::Context(context) => function.call((context,)),
                    Call::Plain => function.call(()),
                }
            })();
            result.map_err(|e| execution_error(&ctx, e, &self.interrupted, self.budget))
        });
        let result = result.and_then(|json| {
            if json.len() > TREE_BYTES {
                return Err("Visual component tree exceeds 1 MiB".into());
            }
            let node: Node = serde_json::from_str(&json)
                .map_err(|e| format!("Invalid visual component tree: {e}"))?;
            node.validate()?;
            Ok(node)
        });
        match &result {
            Ok(node) => self.current = Some(node.clone()),
            Err(_) => self.stopped = true,
        }
        result
    }
}

enum Call<'a> {
    Plain,
    Event(&'a str, &'a str),
    Context(&'a str),
}

fn validate_event(node: &Node, id: &str, value: &serde_json::Value) -> Option<Result<(), String>> {
    let accepted = match node {
        Node::Column { children, .. }
        | Node::Row { children, .. }
        | Node::Grid { children, .. }
        | Node::Card { children, .. } => {
            return children
                .iter()
                .find_map(|child| validate_event(child, id, value));
        }
        Node::Button {
            handler, disabled, ..
        } if handler == id => {
            if *disabled {
                return Some(Err("This visual control is disabled".into()));
            }
            value.is_null()
        }
        Node::Toggle { handler, .. } if handler == id => value.is_boolean(),
        Node::Input { handler, .. } if handler == id => {
            value.as_str().is_some_and(|s| s.len() <= 4096)
        }
        Node::Select {
            handler, options, ..
        } if handler == id => value.as_u64().is_some_and(|v| v < options.len() as u64),
        Node::Tabs {
            handler, labels, ..
        } if handler == id => value.as_u64().is_some_and(|v| v < labels.len() as u64),
        Node::Slider {
            handler, min, max, ..
        } if handler == id => value
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= *min && v <= *max),
        _ => return None,
    };
    Some(if accepted {
        Ok(())
    } else {
        Err("Invalid value for this visual control".into())
    })
}

/// Validate the initial view and a bounded sample of its interactive controls.
/// Each event runs in a fresh VM so checking never changes the published state.
pub fn check(source: &str) -> Result<Node, String> {
    let initial = VisualRuntime::new(source)?.render()?;
    let mut probes = Vec::new();
    collect_probes(&initial, &mut probes);
    for (handler, value) in probes.into_iter().take(8) {
        let mut probe = VisualRuntime::new(source)?;
        probe.render()?;
        probe
            .dispatch(&handler, value)
            .map_err(|e| format!("Visual control check failed ({handler}): {e}"))?;
    }
    Ok(initial)
}

fn collect_probes(node: &Node, probes: &mut Vec<(String, serde_json::Value)>) {
    use serde_json::json;
    if probes.len() >= 8 {
        return;
    }
    match node {
        Node::Column { children, .. }
        | Node::Row { children, .. }
        | Node::Grid { children, .. }
        | Node::Card { children, .. } => {
            for child in children {
                collect_probes(child, probes);
            }
        }
        Node::Button {
            handler,
            disabled: false,
            ..
        } => probes.push((handler.clone(), json!(null))),
        Node::Toggle { handler, value, .. } => probes.push((handler.clone(), json!(!value))),
        Node::Input { handler, value, .. } => probes.push((handler.clone(), json!(value))),
        Node::Select {
            handler,
            options,
            selected,
            ..
        } => probes.push((handler.clone(), json!((selected + 1) % options.len()))),
        Node::Tabs {
            handler,
            labels,
            selected,
            ..
        } => probes.push((handler.clone(), json!((selected + 1) % labels.len()))),
        Node::Slider {
            handler, min, max, ..
        } => {
            probes.push((handler.clone(), json!(min)));
            probes.push((handler.clone(), json!(max)));
        }
        _ => {}
    }
}

fn execution_error(
    ctx: &rquickjs::Ctx<'_>,
    error: rquickjs::Error,
    interrupted: &Cell<bool>,
    budget: Duration,
) -> String {
    let timed_out = || {
        format!(
            "Visual execution exceeded its {} ms time budget",
            budget.as_millis()
        )
    };
    if interrupted.get() {
        // Clear the exception without invoking an untrusted exception accessor.
        let _ = ctx.catch();
        return timed_out();
    }
    let message = if error.is_exception() {
        let exception = ctx.catch();
        exception
            .as_exception()
            .and_then(|e| e.message())
            .unwrap_or_else(|| "Script threw an exception".into())
    } else {
        error.to_string()
    };
    // Reading an Error.message getter can execute script too; it runs under
    // the same interrupt deadline as the operation that threw the exception.
    if interrupted.get() {
        let _ = ctx.catch();
        return timed_out();
    }
    format!(
        "Visual execution failed: {}",
        message.chars().take(2048).collect::<String>()
    )
}

#[cfg(test)]
mod tests;
