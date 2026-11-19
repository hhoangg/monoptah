//! The true shape of a mentioned HTML file, measured before anything expensive runs.
//!
//! The style inliner and the sanitizer build a tree with `html5ever`, whose tree builder may
//! create far more elements than the source has tags: the adoption agency algorithm and the
//! reconstruction of formatting elements clone `<b>`, `<i>`, `<a>`... for every following
//! paragraph (43 KB of source can become two million elements). No count of the source text
//! sees that. This pre-pass runs the same parser into a sink that stores nothing but ids, parent
//! and child links, so it measures the real tree: the element count (clones included), the
//! depth, the widest node, the class/id/tag words (for [`super::cost`]), the `<style>` texts and
//! the linked stylesheet references, in the order and by the rules `css-inline` uses.
//!
//! The sink cannot refuse a tree builder call, so it stops the parse by unwinding as soon as the
//! tree grows past [`MAX_ELEMENTS`] (the clones are created one at a time, so the work done is
//! bounded by the limit, not by the clones the document asks for).
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::HashMap,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    rc::Rc,
};

use html5ever::{
    expanded_name, local_name, ns, parse_document,
    tendril::{StrTendril, TendrilSink},
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
    Attribute, ExpandedName, ParseOpts, QualName,
};

/// Most elements (clones included) a document may build.
pub const MAX_ELEMENTS: usize = 20_000;
/// Deepest nesting a document may build: the inliner and the sanitizer recurse on it.
pub const MAX_DEPTH: usize = 256;
/// Most link edits the tree builder may make (moves of children on a foster parent or an
/// adoption agency run), so the sink's own bookkeeping stays bounded too.
const MAX_WORK: usize = 40_000_000;
/// Most distinct words kept; past it every rule counts as reaching everything.
const MAX_WORDS: usize = 200_000;
/// The source is fed to the parser in slices of this many bytes.
const SLICE_BYTES: usize = 16 * 1024;
const NONE: usize = usize::MAX;

/// What the pre-pass measured.
#[derive(Debug, Default)]
pub struct Shape {
    pub elements: u64,
    pub max_depth: u64,
    pub max_children: u64,
    /// Occurrences of every tag name, class token and id (lowercase).
    pub words: HashMap<String, u64>,
    /// More words than [`MAX_WORDS`].
    pub opaque: bool,
    /// The text of every `<style>` element, in document order.
    pub styles: Vec<String>,
    /// The `href` of every `<link rel=stylesheet>` the inliner would load.
    pub stylesheets: Vec<String>,
}

/// The document is too big or too deep to transform.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused;

struct Overflow;

struct Node {
    id: usize,
    name: Option<QualName>,
    contents: Option<Handle>,
    integration_point: bool,
}

type Handle = Rc<Node>;

#[derive(Default)]
struct State {
    parent: Vec<usize>,
    children: Vec<Vec<usize>>,
    shape: Shape,
    style_of: HashMap<usize, usize>,
    work: usize,
}

impl State {
    fn node(&mut self) -> usize {
        self.parent.push(NONE);
        self.children.push(Vec::new());
        self.parent.len() - 1
    }

    fn word(&mut self, word: &str) {
        if let Some(count) = self.shape.words.get_mut(word) {
            *count += 1;
        } else if self.shape.words.len() < MAX_WORDS {
            self.shape.words.insert(word.to_owned(), 1);
        } else {
            self.shape.opaque = true;
        }
    }

    fn attributes(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            if attr.name.local == local_name!("class") {
                for token in attr.value.split_ascii_whitespace() {
                    self.word(&token.to_lowercase());
                }
            } else if attr.name.local == local_name!("id") {
                self.word(&attr.value.to_lowercase());
            }
        }
    }

    fn detach(&mut self, id: usize) {
        let parent = std::mem::replace(&mut self.parent[id], NONE);
        if parent != NONE {
            let siblings = &mut self.children[parent];
            if let Some(at) = siblings.iter().rposition(|child| *child == id) {
                self.work += siblings.len() - at;
                siblings.remove(at);
            }
        }
    }

    /// Puts `child` in `parent` at `at` (the end when `None`).
    fn attach(&mut self, parent: usize, child: usize, at: Option<usize>) -> Result<(), Overflow> {
        if parent == NONE || child == NONE {
            return Ok(());
        }
        self.detach(child);
        let siblings = &mut self.children[parent];
        self.work += siblings.len() - at.unwrap_or(siblings.len()) + 1;
        match at {
            Some(at) => siblings.insert(at, child),
            None => siblings.push(child),
        }
        self.parent[child] = parent;
        if self.work > MAX_WORK {
            return Err(Overflow);
        }
        Ok(())
    }
}

struct Sink {
    state: Rc<RefCell<State>>,
    document: Handle,
}

fn bail() -> ! {
    resume_unwind(Box::new(Overflow))
}

impl Sink {
    fn make(&self, name: Option<QualName>, template: bool, integration_point: bool) -> Handle {
        let mut state = self.state.borrow_mut();
        let id = state.node();
        drop(state);
        let contents = template.then(|| self.make(None, false, false));
        Rc::new(Node {
            id,
            name,
            contents,
            integration_point,
        })
    }

    fn attach_node(&self, parent: &Handle, child: &Handle, at: Option<usize>) {
        let result = self.state.borrow_mut().attach(parent.id, child.id, at);
        if result.is_err() {
            bail();
        }
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = ();
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) {}

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.document.clone()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        match &target.name {
            Some(name) => name.expanded(),
            None => panic!("not an element"),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let ignored = attrs
            .iter()
            .any(|a| &*a.name.local == "data-css-inline" && &*a.value == "ignore");
        let handle;
        {
            let mut state = self.state.borrow_mut();
            state.shape.elements += 1;
            let over = state.shape.elements > MAX_ELEMENTS as u64;
            if !over {
                state.word(&name.local.to_lowercase());
                state.attributes(&attrs);
            }
            drop(state);
            if over {
                bail();
            }
            handle = self.make(
                Some(name.clone()),
                flags.template,
                flags.mathml_annotation_xml_integration_point,
            );
        }
        if !ignored {
            let mut state = self.state.borrow_mut();
            if name.expanded() == expanded_name!(html "style") {
                let index = state.shape.styles.len();
                state.shape.styles.push(String::new());
                state.style_of.insert(handle.id, index);
            } else if name.expanded() == expanded_name!(html "link") {
                let stylesheet = attrs
                    .iter()
                    .any(|a| a.name.local == local_name!("rel") && &*a.value == "stylesheet");
                let href = attrs
                    .iter()
                    .find(|a| a.name.local == local_name!("href") && !a.value.is_empty());
                if let (true, Some(href)) = (stylesheet, href) {
                    state.shape.stylesheets.push(href.value.to_string());
                }
            }
        }
        handle
    }

    fn create_comment(&self, _text: StrTendril) -> Handle {
        // Comments and processing instructions are leaves nobody walks: they get no node.
        Rc::new(Node {
            id: NONE,
            name: None,
            contents: None,
            integration_point: false,
        })
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Handle {
        self.create_comment(StrTendril::new())
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        match child {
            NodeOrText::AppendNode(node) => self.attach_node(parent, &node, None),
            NodeOrText::AppendText(text) => {
                let mut state = self.state.borrow_mut();
                if let Some(index) = state.style_of.get(&parent.id).copied() {
                    state.shape.styles[index].push_str(&text);
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        let has_parent = element.id != NONE && self.state.borrow().parent[element.id] != NONE;
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &Handle) -> Handle {
        target.contents.clone().expect("not a template element")
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        if let NodeOrText::AppendNode(node) = child {
            let (parent, at) = {
                let state = self.state.borrow();
                let parent = state.parent.get(sibling.id).copied().unwrap_or(NONE);
                let at = (parent != NONE)
                    .then(|| {
                        state.children[parent]
                            .iter()
                            .rposition(|c| *c == sibling.id)
                    })
                    .flatten();
                (parent, at)
            };
            if let Some(at) = at {
                let result = self.state.borrow_mut().attach(parent, node.id, Some(at));
                if result.is_err() {
                    bail();
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        // Only the class and id words of an element's attributes matter.
        let mut state = self.state.borrow_mut();
        let _ = target;
        state.attributes(&attrs);
    }

    fn remove_from_parent(&self, target: &Handle) {
        if target.id != NONE {
            self.state.borrow_mut().detach(target.id);
        }
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if node.id == NONE || new_parent.id == NONE {
            return;
        }
        let mut state = self.state.borrow_mut();
        let moved = std::mem::take(&mut state.children[node.id]);
        state.work += moved.len();
        for child in &moved {
            state.parent[*child] = new_parent.id;
        }
        state.children[new_parent.id].extend(moved);
        let over = state.work > MAX_WORK;
        drop(state);
        if over {
            bail();
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        handle.integration_point
    }
}

/// Measures `source`, or refuses it when its tree would exceed the limits.
pub fn scan(source: &str) -> Result<Shape, Refused> {
    let state = Rc::new(RefCell::new(State::default()));
    let document = {
        let mut inner = state.borrow_mut();
        Rc::new(Node {
            id: inner.node(),
            name: None,
            contents: None,
            integration_point: false,
        })
    };
    let sink = Sink {
        state: state.clone(),
        document,
    };
    let parsed = catch_unwind(AssertUnwindSafe(|| {
        let mut parser = parse_document(sink, ParseOpts::default());
        let mut rest = source;
        while !rest.is_empty() {
            let mut cut = SLICE_BYTES.min(rest.len());
            while !rest.is_char_boundary(cut) {
                cut += 1;
            }
            let (slice, tail) = rest.split_at(cut);
            parser.process(StrTendril::from_slice(slice));
            rest = tail;
        }
        parser.finish();
    }));
    if let Err(payload) = parsed {
        // Anything but the limit unwinding is a parser bug: refuse the document too.
        let _ = payload.downcast::<Overflow>();
        return Err(Refused);
    }
    let mut state = Rc::try_unwrap(state).map_err(|_| Refused)?.into_inner();
    let mut max_children = 0;
    let mut max_depth = 0;
    // Depth by an explicit stack from every root (the document and the template contents).
    for root in 0..state.parent.len() {
        if state.parent[root] != NONE {
            continue;
        }
        let mut stack = vec![(root, 0u64)];
        while let Some((id, depth)) = stack.pop() {
            max_depth = max_depth.max(depth);
            max_children = max_children.max(state.children[id].len() as u64);
            for child in &state.children[id] {
                stack.push((*child, depth + 1));
            }
        }
    }
    state.shape.max_depth = max_depth;
    state.shape.max_children = max_children;
    if max_depth > MAX_DEPTH as u64 {
        return Err(Refused);
    }
    Ok(state.shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_elements_depth_words_styles_and_sheets() {
        let shape = scan(
            "<!DOCTYPE html><html><head><link rel=stylesheet href=a.css>\
             <link rel=stylesheet href=''><style>p{color:red}</style></head>\
             <body><div class='A b'><p id=X>hi</p><!-- c --></div></body></html>",
        )
        .unwrap();
        // html, head, link, link, style, body, div, p.
        assert_eq!(shape.elements, 8);
        assert_eq!(shape.max_depth, 4);
        assert_eq!(shape.words["a"], 1);
        assert_eq!(shape.words["b"], 1);
        assert_eq!(shape.words["x"], 1);
        assert_eq!(shape.stylesheets, ["a.css"]);
        assert_eq!(shape.styles, ["p{color:red}"]);
    }

    #[test]
    fn nesting_past_the_depth_limit_is_refused() {
        assert!(scan(&"<div>".repeat(MAX_DEPTH - 8)).is_ok());
        assert!(scan(&"<div>".repeat(2000)).is_err());
    }

    #[test]
    fn the_adoption_agency_clones_are_counted_and_stop_the_parse() {
        let bomb = format!(
            "<p>{}x</p>{}",
            (0..200)
                .map(|n| format!("<b class=c{n}>"))
                .collect::<String>(),
            "<p>x".repeat(10_000)
        );
        assert!(bomb.len() < 50_000);
        let started = std::time::Instant::now();
        assert!(scan(&bomb).is_err());
        assert!(started.elapsed().as_secs() < 3, "{:?}", started.elapsed());
    }

    #[test]
    fn a_wide_page_is_measured_not_refused() {
        let shape = scan(&"<i class=k>x</i>".repeat(5_000)).unwrap();
        // html, head and body are implied.
        assert_eq!(shape.elements, 5_003);
        assert_eq!(shape.words["k"], 5_000);
        assert!(shape.max_children >= 5_000);
    }
}
