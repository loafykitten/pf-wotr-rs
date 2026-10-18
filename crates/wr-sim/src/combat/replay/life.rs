//! The structure of a root rule's span, as `Rules.cs` writes it: the rules open (a stack, each
//! with its stage), the handlers open (one global stack, an ancestor's staying open across a rule
//! it triggers) and the ids allocated in order. One state machine shared by `span_ok` (the
//! commands' attack spans, which check every field besides) and the audit of the other root
//! spans (`spans.rs`), so the two cannot drift.
//!
//! A rule is pushed (`rule`: its `parent` the innermost open rule, its `depth` the stack's, its
//! `handler` the open handler, in the parent's `OnTrigger` or one of its handlers), then steps
//! once each in turn, by the innermost rule and not while its own handler is open: `about` (stage
//! 1), `trig` (2), `trig_end` (3), `did` (4), `rule_end` (5, popped). Handlers of the innermost
//! rule open and close in order (`mod_before` before `about`, `about` after it, `mod_after` then
//! `did` after `trig_end`), each id above the last.

/// An open handler.
#[derive(Debug, Clone)]
pub(super) struct Handler {
    pub(super) id: i64,
    /// The rule it handles.
    pub(super) rule: i64,
    pub(super) class: String,
    pub(super) phase: String,
}

/// An open rule: its id, its stage (0 pushed, 1 `about`, 2 `trig`, 3 `trig_end`, 4 `did`) and
/// whether a `did` handler of it has opened (`mod_after` handlers come before those).
#[derive(Debug, Clone)]
struct Level {
    id: i64,
    stage: u8,
    did_handlers: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Life {
    stack: Vec<Level>,
    handlers: Vec<Handler>,
    last_rule: i64,
    last_handler: i64,
}

impl Life {
    /// At the root's record (stage 0).
    pub(super) fn new(root: i64) -> Self {
        Self {
            stack: vec![Level { id: root, stage: 0, did_handlers: false }],
            handlers: Vec::new(),
            last_rule: root,
            last_handler: 0,
        }
    }

    /// The innermost open rule (0 when none) and its stage.
    pub(super) fn rule(&self) -> i64 {
        self.stack.last().map_or(0, |t| t.id)
    }

    pub(super) fn stage(&self) -> u8 {
        self.stack.last().map_or(0, |t| t.stage)
    }

    pub(super) fn is_open(&self, rule: i64) -> bool {
        self.stack.iter().any(|t| t.id == rule)
    }

    /// The innermost open handler (its id 0 when none).
    pub(super) fn handler(&self) -> Option<&Handler> {
        self.handlers.last()
    }

    pub(super) fn handler_id(&self) -> i64 {
        self.handlers.last().map_or(0, |h| h.id)
    }

    /// Whether the innermost open handler is the innermost rule's own.
    pub(super) fn own_handler(&self) -> bool {
        self.handlers.last().is_some_and(|h| h.rule == self.rule())
    }

    pub(super) fn no_handlers(&self) -> bool {
        self.handlers.is_empty()
    }

    /// A record of the innermost rule, in the open handler (`rule` and `h` fields).
    pub(super) fn attributed(&self, rule: Option<i64>, h: Option<i64>) -> bool {
        rule == Some(self.rule()) && h == Some(self.handler_id())
    }

    /// A nested rule's push: pushed by the innermost rule in its computation or one of its
    /// handlers, one level down, in the open handler, its context its parent, its id above the
    /// last rule's.
    pub(super) fn can_push(
        &self,
        id: Option<i64>,
        parent: Option<i64>,
        depth: Option<u64>,
        handler: Option<i64>,
        context_is_parent: bool,
    ) -> bool {
        parent == Some(self.rule())
            && depth == u64::try_from(self.stack.len()).ok()
            && handler == Some(self.handler_id())
            && context_is_parent
            && (self.stage() == 2 || self.own_handler())
            && id.is_some_and(|i| i > self.last_rule)
    }

    pub(super) fn push(&mut self, id: i64) {
        self.last_rule = id;
        self.stack.push(Level { id, stage: 0, did_handlers: false });
    }

    /// A step (`about`, `trig`, `trig_end`, `did`, `rule_end`) of the innermost rule at stage
    /// `want`, not while its own handler is open.
    pub(super) fn can_step(&self, id: Option<i64>, want: u8) -> bool {
        id == Some(self.rule()) && self.stage() == want && !self.own_handler()
    }

    pub(super) fn advance(&mut self, next: u8) {
        if let Some(t) = self.stack.last_mut() {
            t.stage = next;
        }
    }

    /// The innermost rule's `rule_end` done.
    pub(super) fn end_rule(&mut self) {
        self.stack.pop();
    }

    /// A handler of the innermost rule, in the phase its stage allows, not inside another of the
    /// rule's own, its id above the last handler's.
    pub(super) fn can_open(&self, id: Option<i64>, rule: Option<i64>, phase: &str) -> bool {
        let did_handlers = self.stack.last().is_some_and(|t| t.did_handlers);
        let phase_ok = match (phase, self.stage()) {
            ("mod_before", 0) | ("about", 1) | ("did", 3) => true,
            ("mod_after", 3) => !did_handlers,
            _ => false,
        };
        rule == Some(self.rule())
            && !self.own_handler()
            && phase_ok
            && id.is_some_and(|i| i > self.last_handler)
    }

    pub(super) fn open(&mut self, id: i64, class: &str, phase: &str) {
        let rule = self.rule();
        if phase == "did"
            && let Some(t) = self.stack.last_mut()
        {
            t.did_handlers = true;
        }
        self.last_handler = id;
        self.handlers.push(Handler { id, rule, class: class.to_owned(), phase: phase.to_owned() });
    }

    /// A handler's end: the open handler of the innermost rule, with its id and rule.
    pub(super) fn can_close(&self, id: Option<i64>, rule: Option<i64>) -> bool {
        self.own_handler() && id == Some(self.handler_id()) && rule == Some(self.rule())
    }

    pub(super) fn close(&mut self) {
        self.handlers.pop();
    }
}
