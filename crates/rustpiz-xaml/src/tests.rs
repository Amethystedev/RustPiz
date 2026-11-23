//! Tests du moteur sur un arbre simulé.

use crate::{Engine, Mode, XamlBackend, MAX_WRITE_FAILURES};
use rustpiz_theme::expr::Value;
use rustpiz_theme::{compile, ControlStyle, Theme};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default)]
struct Node {
    class: String,
    name: String,
    parent: Option<usize>,
    index: u32,
    state: HashMap<String, String>,
    values: HashMap<String, String>,
}

#[derive(Default)]
struct World {
    nodes: Vec<Node>,
    writes: u32,
    fail_property: Option<String>,
    fail_parse: Option<String>,
    fail_resolve: Option<String>,
    active_watches: i32,
}

#[derive(Clone)]
struct Mock(Rc<RefCell<World>>);

impl Mock {
    fn value(&self, id: usize, property: &str) -> Option<String> {
        self.0.borrow().nodes[id].values.get(property).cloned()
    }
}

impl XamlBackend for Mock {
    type Element = usize;
    type Key = usize;
    type Property = String;
    type Value = String;
    type Original = Option<String>;
    type Token = ();

    fn key(&self, element: &usize) -> usize {
        *element
    }
    fn class_name(&self, element: &usize) -> String {
        self.0.borrow().nodes[*element].class.clone()
    }
    fn element_name(&self, element: &usize) -> String {
        self.0.borrow().nodes[*element].name.clone()
    }
    fn parent(&self, element: &usize) -> Option<usize> {
        self.0.borrow().nodes[*element].parent
    }
    fn child_index(&self, element: &usize) -> Option<u32> {
        Some(self.0.borrow().nodes[*element].index)
    }
    fn local_value_equals(&mut self, e: &usize, _: &str, p: &str, expected: &str) -> bool {
        self.0.borrow().nodes[*e].values.get(p).map(String::as_str) == Some(expected)
    }
    fn visual_state(&mut self, owner: &usize, group: &str) -> Option<String> {
        self.0.borrow().nodes[*owner].state.get(group).cloned()
    }
    fn resolve_property(&mut self, _: &usize, _: &str, p: &str) -> Result<String, String> {
        if self.0.borrow().fail_resolve.as_deref() == Some(p) {
            return Err("inconnue".into());
        }
        Ok(p.to_owned())
    }
    fn parse_value(
        &mut self,
        _: &usize,
        _: &str,
        p: &str,
        v: &str,
        _: bool,
    ) -> Result<String, String> {
        if self.0.borrow().fail_parse.as_deref() == Some(p) {
            return Err("illisible".into());
        }
        Ok(v.to_owned())
    }
    fn read_original(&mut self, e: &usize, p: &String) -> Result<Option<String>, String> {
        Ok(self.0.borrow().nodes[*e].values.get(p).cloned())
    }
    fn set_value(&mut self, e: &usize, p: &String, v: &String, _: bool) -> Result<(), String> {
        let mut world = self.0.borrow_mut();
        if world.fail_property.as_deref() == Some(p.as_str()) {
            return Err("refusé".into());
        }
        world.writes += 1;
        world.nodes[*e].values.insert(p.clone(), v.clone());
        Ok(())
    }
    fn restore(&mut self, e: &usize, p: &String, o: Option<String>) -> Result<(), String> {
        let mut world = self.0.borrow_mut();
        match o {
            Some(v) => world.nodes[*e].values.insert(p.clone(), v),
            None => world.nodes[*e].values.remove(p),
        };
        Ok(())
    }
    fn read_primitive(&mut self, e: &usize, p: &String) -> Option<Value> {
        self.0.borrow().nodes[*e]
            .values
            .get(p)
            .cloned()
            .map(Value::Str)
    }
    fn watch_property(&mut self, _: &usize, _: &String, _: &usize) -> Option<()> {
        self.0.borrow_mut().active_watches += 1;
        Some(())
    }
    fn watch_group(&mut self, _: &usize, _: &str, _: &usize) -> Option<()> {
        self.0.borrow_mut().active_watches += 1;
        Some(())
    }
    fn unwatch(&mut self, _: ()) {
        self.0.borrow_mut().active_watches -= 1;
    }
}

const GRID: &str = "Windows.UI.Xaml.Controls.Grid";
const BORDER: &str = "Windows.UI.Xaml.Controls.Border";

struct Fixture {
    mock: Mock,
}

impl Fixture {
    fn new() -> Self {
        Self {
            mock: Mock(Rc::default()),
        }
    }

    fn node(&self, class: &str, name: &str, parent: Option<usize>, index: u32) -> usize {
        let mut world = self.mock.0.borrow_mut();
        world.nodes.push(Node {
            class: class.to_owned(),
            name: name.to_owned(),
            parent,
            index,
            ..Node::default()
        });
        world.nodes.len() - 1
    }

    fn engine(&self, rules: &[(&str, &[&str])], mode: Mode) -> Engine<Mock> {
        let theme = Theme {
            schema: 1,
            style_constants: vec![],
            theme_resource_variables: vec![],
            meta: Default::default(),
            compat: Default::default(),
            control_styles: rules
                .iter()
                .map(|(target, styles)| ControlStyle {
                    target: target.to_string(),
                    styles: styles.iter().map(ToString::to_string).collect(),
                })
                .collect(),
        };
        let (compiled, diagnostics) = compile(&theme, &[]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        Engine::new(self.mock.clone(), compiled.rules, mode)
    }
}

#[test]
fn rollback_restores_existing_and_absent_values() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    f.mock.0.borrow_mut().nodes[a]
        .values
        .insert("Margin".into(), "1".into());
    let mut engine = f.engine(&[("Grid#A", &["Margin=9", "Width=70"])], Mode::Apply);

    engine.on_element_added(a);
    assert_eq!(f.mock.value(a, "Margin").as_deref(), Some("9"));
    assert_eq!(f.mock.value(a, "Width").as_deref(), Some("70"));

    engine.rollback_all();
    assert_eq!(f.mock.value(a, "Margin").as_deref(), Some("1"));
    assert_eq!(f.mock.value(a, "Width"), None);
    assert_eq!(f.mock.0.borrow().active_watches, 0);
}

#[test]
fn hierarchy_wildcard_root_and_index() {
    let f = Fixture::new();
    let root = f.node(GRID, "Root", None, 0);
    let middle = f.node(GRID, "Mid", Some(root), 3);
    let leaf = f.node(BORDER, "L", Some(middle), 1);
    let mut engine = f.engine(
        &[
            ("Grid#Root > Grid > Border#L", &["A=direct"]),
            ("Grid#Root > * > Border#L", &["B=wild"]),
            ("Grid#Mid > * > Border#L", &["C=zero"]),
            (":root > Grid > Grid > Border", &["D=root"]),
            ("Grid[4] > Border[2]", &["E=index"]),
            ("Grid#Nope > Border#L", &["F=no"]),
            (":root > Grid > Border", &["G=no"]),
            ("Grid[1] > Border", &["H=no"]),
        ],
        Mode::Apply,
    );
    engine.on_element_added(leaf);
    for (property, expected) in [
        ("A", Some("direct")),
        ("B", Some("wild")),
        ("C", Some("zero")),
        ("D", Some("root")),
        ("E", Some("index")),
        ("F", None),
        ("G", None),
        ("H", None),
    ] {
        assert_eq!(
            f.mock.value(leaf, property).as_deref(),
            expected,
            "{property}"
        );
    }
    assert_eq!(engine.stats()[5].matched, 0);
    assert_eq!(engine.stats()[0].matched, 1);
}

#[test]
fn property_filter_on_selector() {
    let f = Fixture::new();
    let a = f.node(GRID, "", None, 0);
    let b = f.node(GRID, "", None, 0);
    f.mock.0.borrow_mut().nodes[a]
        .values
        .insert("Key".into(), "yes".into());
    let mut engine = f.engine(&[("Grid[Key=yes]", &["X=1"])], Mode::Apply);
    engine.on_element_added(a);
    engine.on_element_added(b);
    assert_eq!(f.mock.value(a, "X").as_deref(), Some("1"));
    assert_eq!(f.mock.value(b, "X"), None);
}

#[test]
fn visual_states_select_values_and_restore() {
    let f = Fixture::new();
    let owner = f.node(GRID, "O", None, 0);
    let leaf = f.node(BORDER, "L", Some(owner), 0);
    f.mock.0.borrow_mut().nodes[owner]
        .state
        .insert("G".into(), "Normal".into());
    let mut engine = f.engine(
        &[(
            "Grid#O@G > Border#L",
            &["Width=1", "Width@Hover=2", "Only@Hover=3"],
        )],
        Mode::Apply,
    );
    engine.on_element_added(leaf);
    assert_eq!(f.mock.value(leaf, "Width").as_deref(), Some("1"));
    assert_eq!(f.mock.value(leaf, "Only"), None);

    f.mock.0.borrow_mut().nodes[owner]
        .state
        .insert("G".into(), "Hover".into());
    engine.on_state_changed(&leaf, "G");
    assert_eq!(f.mock.value(leaf, "Width").as_deref(), Some("2"));
    assert_eq!(f.mock.value(leaf, "Only").as_deref(), Some("3"));

    f.mock.0.borrow_mut().nodes[owner]
        .state
        .insert("G".into(), "Normal".into());
    engine.on_state_changed(&leaf, "G");
    assert_eq!(f.mock.value(leaf, "Width").as_deref(), Some("1"));
    assert_eq!(f.mock.value(leaf, "Only"), None);
}

#[test]
fn write_failure_undoes_the_element_only() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    let b = f.node(BORDER, "B", None, 0);
    f.mock.0.borrow_mut().fail_property = Some("Bad".into());
    let mut engine = f.engine(
        &[("Grid#A", &["Good=1", "Bad=2"]), ("Border#B", &["Good=1"])],
        Mode::Apply,
    );
    engine.on_element_added(a);
    engine.on_element_added(b);
    assert_eq!(f.mock.value(a, "Good"), None, "transaction annulée");
    assert_eq!(f.mock.value(b, "Good").as_deref(), Some("1"));
    assert_eq!(engine.stats()[0].failed, 1);
    assert!(engine.stats()[0]
        .last_error
        .as_deref()
        .unwrap()
        .contains("Bad"));
}

#[test]
fn parse_failure_skips_only_that_property() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    f.mock.0.borrow_mut().fail_parse = Some("Bad".into());
    let mut engine = f.engine(&[("Grid#A", &["Good=1", "Bad=2"])], Mode::Apply);
    engine.on_element_added(a);
    assert_eq!(f.mock.value(a, "Good").as_deref(), Some("1"));
    assert_eq!(engine.stats()[0].failed, 1);
}

#[test]
fn unresolved_property_is_reported() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    f.mock.0.borrow_mut().fail_resolve = Some("Bad".into());
    let mut engine = f.engine(&[("Grid#A", &["Bad=2", "Good=1"])], Mode::Apply);
    engine.on_element_added(a);
    assert_eq!(f.mock.value(a, "Good").as_deref(), Some("1"));
    assert_eq!(engine.stats()[0].failed, 1);
}

#[test]
fn circuit_breaker_restores_everything_and_stops() {
    let f = Fixture::new();
    let good = f.node(BORDER, "G", None, 0);
    let mut engine = f.engine(
        &[("Border#G", &["Ok=1"]), ("Grid", &["Bad=2"])],
        Mode::Apply,
    );
    engine.on_element_added(good);
    assert_eq!(f.mock.value(good, "Ok").as_deref(), Some("1"));

    f.mock.0.borrow_mut().fail_property = Some("Bad".into());
    for _ in 0..MAX_WRITE_FAILURES {
        let node = f.node(GRID, "", None, 0);
        engine.on_element_added(node);
    }
    assert!(engine.tripped());
    assert_eq!(f.mock.value(good, "Ok"), None);

    let after = f.node(BORDER, "G", None, 0);
    engine.on_element_added(after);
    assert_eq!(f.mock.value(after, "Ok"), None, "moteur arrêté");
}

#[test]
fn dry_run_never_writes_but_reports() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    let mut engine = f.engine(
        &[("Grid#A", &["X=1"]), ("Grid#Missing", &["Y=1"])],
        Mode::DryRun,
    );
    engine.on_element_added(a);
    assert_eq!(f.mock.0.borrow().writes, 0);
    assert_eq!(f.mock.value(a, "X"), None);
    assert_eq!(engine.stats()[0].applied, 1);
    assert_eq!(engine.stats()[1].matched, 0);
    engine.rollback_all();
    assert_eq!(f.mock.0.borrow().writes, 0);
}

#[test]
fn surface_capture_drives_dynamic_values() {
    let f = Fixture::new();
    let root = f.node("Taskbar.TaskbarFrame", "", None, 0);
    let grid = f.node(GRID, "RootGrid", Some(root), 0);
    let tray = f.node("SystemTray.SystemTrayFrame", "", None, 0);
    f.mock.0.borrow_mut().nodes[grid]
        .state
        .insert("DockingStates".into(), "".into());
    let mut engine = f.engine(
        &[
            (
                "Taskbar.TaskbarFrame > Grid#RootGrid@DockingStates",
                &[
                    "Tag=horizontal",
                    "Tag@DockedLeft=vertical",
                    "Tag=>taskbarDock",
                ],
            ),
            (
                "SystemTray.SystemTrayFrame",
                &["Height={{taskbarDock==`vertical`?skip():70}}"],
            ),
        ],
        Mode::Apply,
    );
    engine.on_element_added(tray);
    assert_eq!(
        f.mock.value(tray, "Height").as_deref(),
        Some("70"),
        "variable absente"
    );

    engine.on_element_added(grid);
    assert_eq!(f.mock.value(grid, "Tag").as_deref(), Some("horizontal"));
    assert_eq!(f.mock.value(tray, "Height").as_deref(), Some("70"));

    f.mock.0.borrow_mut().nodes[grid]
        .state
        .insert("DockingStates".into(), "DockedLeft".into());
    engine.on_state_changed(&grid, "DockingStates");
    assert_eq!(f.mock.value(grid, "Tag").as_deref(), Some("vertical"));
    assert_eq!(
        f.mock.value(tray, "Height"),
        None,
        "skip() restaure l'origine"
    );

    f.mock.0.borrow_mut().nodes[grid]
        .state
        .insert("DockingStates".into(), "".into());
    engine.on_state_changed(&grid, "DockingStates");
    assert_eq!(f.mock.value(tray, "Height").as_deref(), Some("70"));

    engine.on_element_removed(&grid);
    assert_eq!(f.mock.value(tray, "Height").as_deref(), Some("70"));
    engine.rollback_all();
    assert_eq!(f.mock.value(tray, "Height"), None);
}

#[test]
fn nearest_capture_wins() {
    let f = Fixture::new();
    let top = f.node(GRID, "T", None, 0);
    let left = f.node(GRID, "L", Some(top), 0);
    let right = f.node(GRID, "R", Some(top), 1);
    let capture_left = f.node(BORDER, "C", Some(left), 0);
    let capture_right = f.node(BORDER, "C", Some(right), 0);
    let consumer = f.node(BORDER, "U", Some(left), 1);
    f.mock.0.borrow_mut().nodes[capture_left]
        .values
        .insert("Tag".into(), "left".into());
    f.mock.0.borrow_mut().nodes[capture_right]
        .values
        .insert("Tag".into(), "right".into());
    let mut engine = f.engine(
        &[("Border#C", &["Tag=>v"]), ("Border#U", &["W={{v}}"])],
        Mode::Apply,
    );
    engine.on_element_added(capture_left);
    engine.on_element_added(capture_right);
    engine.on_element_added(consumer);
    assert_eq!(f.mock.value(consumer, "W").as_deref(), Some("left"));
}

#[test]
fn external_change_is_adopted_as_the_new_origin() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    f.mock.0.borrow_mut().nodes[a]
        .values
        .insert("W".into(), "orig".into());
    let mut engine = f.engine(&[("Grid#A", &["W=mine"])], Mode::Apply);
    engine.on_element_added(a);

    f.mock.0.borrow_mut().nodes[a]
        .values
        .insert("W".into(), "external".into());
    engine.on_property_changed(&a, &"W".to_owned());
    assert_eq!(f.mock.value(a, "W").as_deref(), Some("mine"));

    engine.rollback_all();
    assert_eq!(f.mock.value(a, "W").as_deref(), Some("external"));
}

#[test]
fn removed_element_is_not_written_and_readd_is_clean() {
    let f = Fixture::new();
    let a = f.node(GRID, "A", None, 0);
    f.mock.0.borrow_mut().nodes[a]
        .values
        .insert("W".into(), "orig".into());
    let mut engine = f.engine(&[("Grid#A", &["W=mine"])], Mode::Apply);
    engine.on_element_added(a);
    engine.on_element_added(a);
    assert_eq!(f.mock.value(a, "W").as_deref(), Some("mine"));
    engine.rollback_all();
    assert_eq!(
        f.mock.value(a, "W").as_deref(),
        Some("orig"),
        "pas d'origine écrasée"
    );
}
