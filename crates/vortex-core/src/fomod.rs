//! FOMOD installer: required files, option groups, and conditional steps.
//!
//! A group that still has a real choice is reported so the shell can ask.
//! Required and recommended options are preselected. Flags set by a selected
//! option apply to later groups and steps. `gameDependency`, `fommDependency`,
//! and script-extender checks are treated as satisfied.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::deploy::{copy_file, resolve_case, safe_relative, ModError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FomodNote {
    pub group: String,
    pub plugin: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FomodInstall {
    pub name: String,
    pub notes: Vec<FomodNote>,
}

/// One group of options the user picked. Names match the installer XML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FomodChoice {
    pub step: String,
    pub group: String,
    pub plugins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FomodPlan {
    pub name: String,
    pub needs_choices: bool,
    pub steps: Vec<FomodStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FomodStep {
    pub name: String,
    pub visible: bool,
    pub valid: bool,
    pub groups: Vec<FomodGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FomodGroup {
    pub name: String,
    pub select: String,
    pub hint: String,
    pub valid: bool,
    pub problem: String,
    pub plugins: Vec<FomodOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FomodOption {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub kind: String,
    pub selected: bool,
    pub locked: bool,
}

/// Files in the game `Data` folder, and which plugins are enabled.
///
/// A plugin file that exists and is enabled is Active. A plugin file that
/// exists but is disabled is Inactive. Anything else is Missing. A loose file
/// that exists counts as Active.
#[derive(Debug, Clone)]
pub struct InstalledFiles {
    data: PathBuf,
    enabled: Vec<String>,
}

impl InstalledFiles {
    pub fn new(data: impl Into<PathBuf>, enabled: Vec<String>) -> Self {
        Self {
            data: data.into(),
            enabled,
        }
    }

    pub fn empty() -> Self {
        Self {
            data: PathBuf::new(),
            enabled: Vec::new(),
        }
    }

    fn presence(&self, spec: &str) -> Presence {
        let Some(path) = self.find(spec) else {
            return Presence::Missing;
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Presence::Active;
        };
        let lower = name.to_ascii_lowercase();
        let plugin = lower.ends_with(".esm") || lower.ends_with(".esp") || lower.ends_with(".esl");
        if !plugin {
            return Presence::Active;
        }
        if self
            .enabled
            .iter()
            .any(|enabled| enabled.eq_ignore_ascii_case(name))
        {
            Presence::Active
        } else {
            Presence::Inactive
        }
    }

    fn find(&self, spec: &str) -> Option<PathBuf> {
        if self.data.as_os_str().is_empty() || !self.data.is_dir() {
            return None;
        }
        let normalized = spec.replace('\\', "/");
        let trimmed = normalized.trim().trim_start_matches('/');
        let relative = trimmed
            .strip_prefix("Data/")
            .or_else(|| trimmed.strip_prefix("data/"))
            .unwrap_or(trimmed);
        if let Some(path) = safe_relative(relative).and_then(|path| resolve_case(&self.data, &path))
        {
            if path.is_file() {
                return Some(path);
            }
        }
        let name = Path::new(relative).file_name()?;
        let path = resolve_case(&self.data, Path::new(name))?;
        path.is_file().then_some(path)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Active,
    Inactive,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileState {
    Active,
    Inactive,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Select {
    ExactlyOne,
    AtMostOne,
    AtLeastOne,
    Any,
    All,
}

impl Select {
    fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "selectexactlyone" => Select::ExactlyOne,
            "selectatmostone" => Select::AtMostOne,
            "selectatleastone" => Select::AtLeastOne,
            "selectall" => Select::All,
            _ => Select::Any,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Select::ExactlyOne => "SelectExactlyOne",
            Select::AtMostOne => "SelectAtMostOne",
            Select::AtLeastOne => "SelectAtLeastOne",
            Select::Any => "SelectAny",
            Select::All => "SelectAll",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Select::ExactlyOne => "Pick one",
            Select::AtMostOne => "Pick one or skip",
            Select::AtLeastOne => "Pick at least one",
            Select::Any => "Pick any",
            Select::All => "Included",
        }
    }

    fn problem(self) -> &'static str {
        match self {
            Select::ExactlyOne => "Pick one option.",
            Select::AtMostOne => "Pick at most one option.",
            Select::AtLeastOne => "Pick at least one option.",
            Select::Any => "Check the options on this step.",
            Select::All => "Included options cannot be changed.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operator {
    And,
    Or,
}

enum Check {
    Flag { name: String, value: String },
    File { file: String, state: FileState },
    Satisfied,
    Nested(Deps),
}

struct Deps {
    operator: Operator,
    checks: Vec<Check>,
}

enum TypeDesc {
    Fixed(String),
    Patterns {
        default_kind: String,
        patterns: Vec<(Deps, String)>,
    },
}

#[derive(Clone)]
struct Packed {
    source: String,
    destination: String,
    folder: bool,
    priority: i32,
}

struct PluginSpec {
    name: String,
    description: String,
    image: String,
    files: Vec<Packed>,
    flags: Vec<(String, String)>,
    typed: TypeDesc,
}

struct GroupSpec {
    name: String,
    select: Select,
    plugins: Vec<PluginSpec>,
}

struct StepSpec {
    name: String,
    visible: Option<Deps>,
    groups: Vec<GroupSpec>,
}

struct Conditional {
    dependencies: Deps,
    files: Vec<Packed>,
}

struct Module {
    name: String,
    required: Vec<Packed>,
    steps: Vec<StepSpec>,
    conditional: Vec<Conditional>,
}

struct Outcome {
    plan: FomodPlan,
    files: Vec<Packed>,
    notes: Vec<FomodNote>,
}

pub fn find_module_config(package: &Path) -> Option<PathBuf> {
    find_named(package, "fomod", "ModuleConfig.xml")
}

/// Install with recommended defaults and no knowledge of installed plugins.
pub fn install_fomod(package: &Path, xml: &str, dest: &Path) -> Result<FomodInstall, ModError> {
    install_fomod_with(package, xml, dest, &InstalledFiles::empty(), &[])
}

/// Install `choices`. An empty list uses the recommended defaults.
///
/// A choice that breaks a group's rule returns an error and copies nothing.
pub fn install_fomod_with(
    package: &Path,
    xml: &str,
    dest: &Path,
    files: &InstalledFiles,
    choices: &[FomodChoice],
) -> Result<FomodInstall, ModError> {
    let module = parse_module(xml)?;
    let outcome = evaluate(package, &module, files, choices);
    if !choices.is_empty() {
        let problems = problems_of(&outcome.plan);
        if !problems.is_empty() {
            return Err(ModError::new(problems.join(" ")));
        }
    }
    fs::create_dir_all(dest)?;
    for file in &outcome.files {
        copy_packed(package, dest, file)?;
    }
    Ok(FomodInstall {
        name: outcome.plan.name,
        notes: outcome.notes,
    })
}

/// Resolve which steps and options the installer would show.
pub fn preview_fomod(
    package: &Path,
    xml: &str,
    files: &InstalledFiles,
    choices: &[FomodChoice],
) -> Result<FomodPlan, ModError> {
    let module = parse_module(xml)?;
    Ok(evaluate(package, &module, files, choices).plan)
}

fn problems_of(plan: &FomodPlan) -> Vec<String> {
    let mut problems = Vec::new();
    for step in &plan.steps {
        if !step.visible {
            continue;
        }
        for group in &step.groups {
            if !group.valid {
                problems.push(format!("{} / {}: {}", step.name, group.name, group.problem));
            }
        }
    }
    problems
}

fn parse_module(xml: &str) -> Result<Module, ModError> {
    let xml = xml.trim_start_matches('\u{feff}');
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| ModError::new(format!("FOMOD installer could not be read: {err}")))?;
    let root = document.root_element();
    let name = child_text(root, "moduleName").unwrap_or_else(|| "Mod".to_string());
    let required = files_under(root, "requiredInstallFiles");
    let mut steps = Vec::new();
    if let Some(list) = child(root, "installSteps") {
        let order = list.attribute("order").unwrap_or("Explicit");
        for step in elements(list, "installStep") {
            steps.push(parse_step(step));
        }
        sort_named(&mut steps, order, |step| step.name.as_str());
    }
    let mut conditional = Vec::new();
    if let Some(installs) = child(root, "conditionalFileInstalls") {
        if let Some(patterns) = child(installs, "patterns") {
            for pattern in elements(patterns, "pattern") {
                conditional.push(Conditional {
                    dependencies: child(pattern, "dependencies")
                        .map(parse_deps)
                        .unwrap_or(Deps {
                            operator: Operator::And,
                            checks: Vec::new(),
                        }),
                    files: child(pattern, "files")
                        .map(packed_files)
                        .unwrap_or_default(),
                });
            }
        }
    }
    Ok(Module {
        name,
        required,
        steps,
        conditional,
    })
}

fn parse_step(node: roxmltree::Node<'_, '_>) -> StepSpec {
    let mut groups = Vec::new();
    if let Some(list) = child(node, "optionalFileGroups") {
        let order = list.attribute("order").unwrap_or("Explicit");
        for group in elements(list, "group") {
            groups.push(parse_group(group));
        }
        sort_named(&mut groups, order, |group| group.name.as_str());
    }
    StepSpec {
        name: node.attribute("name").unwrap_or("Step").trim().to_string(),
        visible: child(node, "visible").map(parse_deps),
        groups,
    }
}

fn parse_group(node: roxmltree::Node<'_, '_>) -> GroupSpec {
    let mut plugins = Vec::new();
    if let Some(list) = child(node, "plugins") {
        let order = list.attribute("order").unwrap_or("Explicit").to_string();
        for plugin in elements(list, "plugin") {
            plugins.push(parse_plugin(plugin));
        }
        sort_named(&mut plugins, &order, |plugin| plugin.name.as_str());
    }
    GroupSpec {
        name: node
            .attribute("name")
            .unwrap_or("Options")
            .trim()
            .to_string(),
        select: Select::parse(node.attribute("type").unwrap_or("SelectAny")),
        plugins,
    }
}

fn parse_plugin(node: roxmltree::Node<'_, '_>) -> PluginSpec {
    let mut flags = Vec::new();
    if let Some(list) = child(node, "conditionFlags") {
        for flag in elements(list, "flag") {
            let Some(name) = flag.attribute("name") else {
                continue;
            };
            let value = flag.text().unwrap_or("").trim();
            if !value.is_empty() {
                flags.push((name.to_string(), value.to_string()));
            }
        }
    }
    PluginSpec {
        name: node
            .attribute("name")
            .unwrap_or("Option")
            .trim()
            .to_string(),
        description: child(node, "description")
            .map(node_text)
            .unwrap_or_default(),
        image: child(node, "image")
            .and_then(|image| image.attribute("path"))
            .unwrap_or("")
            .to_string(),
        files: child(node, "files").map(packed_files).unwrap_or_default(),
        flags,
        typed: parse_type(node),
    }
}

fn parse_type(node: roxmltree::Node<'_, '_>) -> TypeDesc {
    let Some(descriptor) = child(node, "typeDescriptor") else {
        return TypeDesc::Fixed("Optional".to_string());
    };
    if let Some(kind) = child(descriptor, "type") {
        return TypeDesc::Fixed(canonical_kind(kind.attribute("name").unwrap_or("Optional")));
    }
    if let Some(dependency) = child(descriptor, "dependencyType") {
        let default_kind = canonical_kind(
            child(dependency, "defaultType")
                .and_then(|kind| kind.attribute("name"))
                .unwrap_or("Optional"),
        );
        let mut patterns = Vec::new();
        if let Some(list) = child(dependency, "patterns") {
            for pattern in elements(list, "pattern") {
                let deps = child(pattern, "dependencies")
                    .map(parse_deps)
                    .unwrap_or(Deps {
                        operator: Operator::And,
                        checks: Vec::new(),
                    });
                let kind = canonical_kind(
                    child(pattern, "type")
                        .and_then(|kind| kind.attribute("name"))
                        .unwrap_or("Optional"),
                );
                patterns.push((deps, kind));
            }
        }
        return TypeDesc::Patterns {
            default_kind,
            patterns,
        };
    }
    TypeDesc::Fixed("Optional".to_string())
}

fn parse_deps(node: roxmltree::Node<'_, '_>) -> Deps {
    let operator = if node
        .attribute("operator")
        .unwrap_or("And")
        .eq_ignore_ascii_case("or")
    {
        Operator::Or
    } else {
        Operator::And
    };
    let mut checks = Vec::new();
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        match child.tag_name().name().to_ascii_lowercase().as_str() {
            "flagdependency" => checks.push(Check::Flag {
                name: child.attribute("flag").unwrap_or("").to_string(),
                value: child.attribute("value").unwrap_or("").to_string(),
            }),
            "filedependency" => checks.push(Check::File {
                file: child.attribute("file").unwrap_or("").to_string(),
                state: match child
                    .attribute("state")
                    .unwrap_or("Missing")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "active" => FileState::Active,
                    "inactive" => FileState::Inactive,
                    _ => FileState::Missing,
                },
            }),
            "gamedependency" | "fommdependency" | "scriptextenderdependency" | "fosedependency" => {
                checks.push(Check::Satisfied);
            }
            "dependencies" => checks.push(Check::Nested(parse_deps(child))),
            _ => {}
        }
    }
    Deps { operator, checks }
}

fn evaluate(
    package: &Path,
    module: &Module,
    files: &InstalledFiles,
    choices: &[FomodChoice],
) -> Outcome {
    let mut flags = HashMap::new();
    let mut steps = Vec::new();
    let mut packed = module.required.clone();
    let mut notes = Vec::new();
    for step in &module.steps {
        let visible = step
            .visible
            .as_ref()
            .is_none_or(|deps| eval_deps(deps, &flags, files));
        if !visible {
            steps.push(FomodStep {
                name: step.name.clone(),
                visible: false,
                valid: true,
                groups: Vec::new(),
            });
            continue;
        }
        let mut groups = Vec::new();
        let mut step_valid = true;
        for group in &step.groups {
            let kinds: Vec<String> = group
                .plugins
                .iter()
                .map(|plugin| resolve_kind(plugin, &flags, files))
                .collect();
            let choice = find_choice(choices, &step.name, &group.name);
            let (indices, valid) = decide(group.select, &kinds, &group.plugins, choice);
            if !valid {
                step_valid = false;
            }
            let required_count = kinds
                .iter()
                .filter(|kind| kind.as_str() == "Required")
                .count();
            let mut options = Vec::new();
            for (index, plugin) in group.plugins.iter().enumerate() {
                let kind = kinds[index].clone();
                let selected = indices.contains(&index);
                if selected {
                    packed.extend(plugin.files.iter().cloned());
                    notes.push(FomodNote {
                        group: group.name.clone(),
                        plugin: plugin.name.clone(),
                        kind: kind.clone(),
                    });
                    for (name, value) in &plugin.flags {
                        set_flag(&mut flags, name, value);
                    }
                }
                options.push(FomodOption {
                    name: plugin.name.clone(),
                    description: plugin.description.clone(),
                    image: option_image(package, &plugin.image),
                    kind: kind.clone(),
                    selected,
                    locked: locked(group.select, &kind, required_count),
                });
            }
            groups.push(FomodGroup {
                name: group.name.clone(),
                select: group.select.as_str().to_string(),
                hint: group.select.hint().to_string(),
                valid,
                problem: if valid {
                    String::new()
                } else {
                    group.select.problem().to_string()
                },
                plugins: options,
            });
        }
        steps.push(FomodStep {
            name: step.name.clone(),
            visible: true,
            valid: step_valid,
            groups,
        });
    }
    for pattern in &module.conditional {
        if eval_deps(&pattern.dependencies, &flags, files) {
            packed.extend(pattern.files.iter().cloned());
        }
    }
    packed.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.source.cmp(&right.source))
    });
    let needs_choices = steps
        .iter()
        .filter(|step| step.visible)
        .flat_map(|step| &step.groups)
        .any(group_needs_choice);
    Outcome {
        plan: FomodPlan {
            name: module.name.clone(),
            needs_choices,
            steps,
        },
        files: packed,
        notes,
    }
}

fn option_image(package: &Path, source: &str) -> Option<String> {
    let relative = safe_relative(source)?;
    let path = resolve_case(package, &relative)?;
    path.is_file().then(|| path.display().to_string())
}

fn resolve_kind(
    plugin: &PluginSpec,
    flags: &HashMap<String, String>,
    files: &InstalledFiles,
) -> String {
    match &plugin.typed {
        TypeDesc::Fixed(kind) => kind.clone(),
        TypeDesc::Patterns {
            default_kind,
            patterns,
        } => {
            for (deps, kind) in patterns {
                if eval_deps(deps, flags, files) {
                    return kind.clone();
                }
            }
            default_kind.clone()
        }
    }
}

fn find_choice<'a>(choices: &'a [FomodChoice], step: &str, group: &str) -> Option<&'a FomodChoice> {
    choices.iter().find(|choice| {
        choice.step.eq_ignore_ascii_case(step) && choice.group.eq_ignore_ascii_case(group)
    })
}

fn decide(
    select: Select,
    kinds: &[String],
    plugins: &[PluginSpec],
    choice: Option<&FomodChoice>,
) -> (Vec<usize>, bool) {
    let defaults = default_indices(select, kinds);
    let Some(choice) = choice else {
        return (defaults, true);
    };
    let usable = usable_indices(kinds);
    let required = of_kind(kinds, &usable, "Required");
    let mut picked = required.clone();
    for index in &usable {
        if required.contains(index) {
            continue;
        }
        let name = &plugins[*index].name;
        if choice
            .plugins
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(name))
        {
            push_unique(&mut picked, *index);
        }
    }
    match select {
        Select::All => (usable, true),
        Select::ExactlyOne => {
            if !required.is_empty() {
                (vec![required[0]], true)
            } else if picked.len() == 1 {
                (picked, true)
            } else if picked.is_empty() {
                (defaults, false)
            } else {
                (vec![picked[0]], false)
            }
        }
        Select::AtMostOne => {
            if required.len() == 1 {
                (vec![required[0]], true)
            } else if picked.len() <= 1 {
                (picked, true)
            } else {
                (vec![picked[0]], false)
            }
        }
        Select::AtLeastOne => {
            if picked.is_empty() {
                (defaults, false)
            } else {
                (picked, true)
            }
        }
        Select::Any => (picked, true),
    }
}

fn default_indices(select: Select, kinds: &[String]) -> Vec<usize> {
    let usable = usable_indices(kinds);
    let required = of_kind(kinds, &usable, "Required");
    let recommended = of_kind(kinds, &usable, "Recommended");
    let one = |preferred: &[usize]| -> Vec<usize> {
        preferred
            .first()
            .or(usable.first())
            .copied()
            .into_iter()
            .collect()
    };
    match select {
        Select::ExactlyOne => {
            if !required.is_empty() {
                one(&required)
            } else if !recommended.is_empty() {
                one(&recommended)
            } else {
                one(&[])
            }
        }
        Select::AtMostOne => {
            if !required.is_empty() {
                one(&required)
            } else {
                recommended.first().copied().into_iter().collect()
            }
        }
        Select::All => usable,
        Select::AtLeastOne => {
            let mut chosen = required;
            chosen.extend(recommended);
            if chosen.is_empty() {
                one(&[])
            } else {
                chosen
            }
        }
        Select::Any => {
            let mut chosen = required;
            chosen.extend(recommended);
            chosen
        }
    }
}

fn usable_indices(kinds: &[String]) -> Vec<usize> {
    kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| kind.as_str() != "NotUsable")
        .map(|(index, _)| index)
        .collect()
}

fn of_kind(kinds: &[String], indices: &[usize], kind: &str) -> Vec<usize> {
    indices
        .iter()
        .copied()
        .filter(|index| kinds[*index] == kind)
        .collect()
}

fn push_unique(indices: &mut Vec<usize>, index: usize) {
    if !indices.contains(&index) {
        indices.push(index);
    }
}

fn locked(select: Select, kind: &str, required_count: usize) -> bool {
    if kind == "NotUsable" || kind == "Required" || select == Select::All {
        return true;
    }
    if required_count >= 1 && matches!(select, Select::ExactlyOne | Select::AtMostOne) {
        return true;
    }
    false
}

fn group_needs_choice(group: &FomodGroup) -> bool {
    let usable: Vec<&FomodOption> = group
        .plugins
        .iter()
        .filter(|plugin| plugin.kind != "NotUsable")
        .collect();
    let required = usable
        .iter()
        .filter(|plugin| plugin.kind == "Required")
        .count();
    let choosable = usable
        .iter()
        .filter(|plugin| {
            matches!(
                plugin.kind.as_str(),
                "Optional" | "Recommended" | "CouldBeUsable"
            )
        })
        .count();
    match group.select.as_str() {
        "SelectAll" => false,
        "SelectExactlyOne" => required != 1 && usable.len() > 1,
        "SelectAtMostOne" => required == 0 && choosable > 0,
        "SelectAtLeastOne" | "SelectAny" => choosable > 0,
        _ => choosable > 0,
    }
}

fn eval_deps(deps: &Deps, flags: &HashMap<String, String>, files: &InstalledFiles) -> bool {
    if deps.checks.is_empty() {
        return true;
    }
    let check = |item: &Check| eval_check(item, flags, files);
    match deps.operator {
        Operator::And => deps.checks.iter().all(check),
        Operator::Or => deps.checks.iter().any(check),
    }
}

fn eval_check(check: &Check, flags: &HashMap<String, String>, files: &InstalledFiles) -> bool {
    match check {
        Check::Flag { name, value } => {
            flag_value(flags, name).is_some_and(|current| current.eq_ignore_ascii_case(value))
        }
        Check::File { file, state } => files.presence(file) == presence_of(*state),
        Check::Satisfied => true,
        Check::Nested(deps) => eval_deps(deps, flags, files),
    }
}

fn presence_of(state: FileState) -> Presence {
    match state {
        FileState::Active => Presence::Active,
        FileState::Inactive => Presence::Inactive,
        FileState::Missing => Presence::Missing,
    }
}

fn flag_value<'a>(flags: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn set_flag(flags: &mut HashMap<String, String>, name: &str, value: &str) {
    if let Some(key) = flags
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned()
    {
        flags.insert(key, value.to_string());
    } else {
        flags.insert(name.to_string(), value.to_string());
    }
}

fn canonical_kind(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "required" => "Required",
        "recommended" => "Recommended",
        "notusable" => "NotUsable",
        "couldbeusable" => "CouldBeUsable",
        _ => "Optional",
    }
    .to_string()
}

fn sort_named<T>(items: &mut [T], order: &str, name: impl Fn(&T) -> &str) {
    match order.to_ascii_lowercase().as_str() {
        "ascending" => items.sort_by(|left, right| {
            name(left)
                .to_ascii_lowercase()
                .cmp(&name(right).to_ascii_lowercase())
        }),
        "descending" => items.sort_by(|left, right| {
            name(right)
                .to_ascii_lowercase()
                .cmp(&name(left).to_ascii_lowercase())
        }),
        _ => {}
    }
}

fn files_under(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<Packed> {
    child(node, name).map(packed_files).unwrap_or_default()
}

fn packed_files(node: roxmltree::Node<'_, '_>) -> Vec<Packed> {
    let mut files = Vec::new();
    for file in elements(node, "file") {
        if let Some(source) = file.attribute("source") {
            files.push(Packed {
                source: source.to_string(),
                destination: file.attribute("destination").unwrap_or("").to_string(),
                folder: false,
                priority: file
                    .attribute("priority")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
            });
        }
    }
    for folder in elements(node, "folder") {
        if let Some(source) = folder.attribute("source") {
            files.push(Packed {
                source: source.to_string(),
                destination: folder.attribute("destination").unwrap_or("").to_string(),
                folder: true,
                priority: folder
                    .attribute("priority")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
            });
        }
    }
    files
}

fn copy_packed(package: &Path, dest: &Path, file: &Packed) -> Result<(), ModError> {
    let Some(source_rel) = safe_relative(&file.source) else {
        return Err(ModError::new(format!(
            "FOMOD source escapes the archive: {}",
            file.source
        )));
    };
    let Some(source) = resolve_case(package, &source_rel) else {
        return Err(ModError::new(format!(
            "FOMOD source is missing: {}",
            file.source
        )));
    };
    if file.folder {
        let prefix = destination_dir(&file.destination)?;
        for relative in crate::deploy::walk_files(&source)? {
            let target = dest.join(&prefix).join(&relative);
            copy_file(&source.join(&relative), &target)?;
        }
    } else {
        let target = dest.join(destination_file(&file.source, &file.destination)?);
        copy_file(&source, &target)?;
    }
    Ok(())
}

fn destination_dir(destination: &str) -> Result<PathBuf, ModError> {
    let trimmed = destination.trim().trim_matches(['/', '\\']);
    if trimmed.is_empty() {
        return Ok(PathBuf::new());
    }
    safe_relative(trimmed).ok_or_else(|| {
        ModError::new(format!(
            "FOMOD destination escapes the data folder: {destination}"
        ))
    })
}

fn destination_file(source: &str, destination: &str) -> Result<PathBuf, ModError> {
    let trimmed = destination.trim();
    if trimmed.is_empty() || trimmed.ends_with('/') || trimmed.ends_with('\\') {
        let name = Path::new(source)
            .file_name()
            .ok_or_else(|| ModError::new(format!("FOMOD file has no name: {source}")))?;
        return Ok(destination_dir(trimmed)?.join(name));
    }
    safe_relative(trimmed).ok_or_else(|| {
        ModError::new(format!(
            "FOMOD destination escapes the data folder: {destination}"
        ))
    })
}

fn find_named(package: &Path, directory: &str, file: &str) -> Option<PathBuf> {
    let dir = resolve_case(package, Path::new(directory))?;
    resolve_case(&dir, Path::new(file)).filter(|path| path.is_file())
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    elements(node, name).into_iter().next()
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let text = child(node, name)?.text()?.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn node_text(node: roxmltree::Node<'_, '_>) -> String {
    let mut text = String::new();
    for descendant in node.descendants() {
        if descendant.is_text() {
            if let Some(piece) = descendant.text() {
                text.push(' ');
                text.push_str(piece);
            }
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn elements<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Vec<roxmltree::Node<'a, 'input>> {
    node.children()
        .filter(|child| child.is_element() && child.tag_name().name().eq_ignore_ascii_case(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_package(root: &Path) -> PathBuf {
        let package = root.join("package");
        fs::create_dir_all(package.join("fomod")).unwrap();
        fs::create_dir_all(package.join("full")).unwrap();
        fs::create_dir_all(package.join("lite")).unwrap();
        fs::create_dir_all(package.join("tents")).unwrap();
        fs::create_dir_all(package.join("patch")).unwrap();
        fs::write(package.join("fomod/shot.png"), b"png").unwrap();
        fs::write(package.join("full/Campfire.esm"), b"full").unwrap();
        fs::write(package.join("lite/Campfire.esm"), b"lite").unwrap();
        fs::write(package.join("tents/Tents.esp"), b"tents").unwrap();
        fs::write(package.join("patch/FrostfallPatch.esp"), b"patch").unwrap();
        fs::write(package.join("patch/extra.txt"), b"extra").unwrap();
        package
    }

    fn sample_xml() -> &'static str {
        r#"
            <config>
              <moduleName>Campfire</moduleName>
              <installSteps>
                <installStep name="Options">
                  <optionalFileGroups>
                    <group name="Version" type="SelectExactlyOne">
                      <plugins>
                        <plugin name="Full">
                          <description>Camping, crafting, and frostfall support.</description>
                          <image path="fomod/shot.png"/>
                          <conditionFlags><flag name="version">full</flag></conditionFlags>
                          <files><file source="full/Campfire.esm" destination="Campfire.esm"/></files>
                          <typeDescriptor><type name="Recommended"/></typeDescriptor>
                        </plugin>
                        <plugin name="Lite">
                          <description>The camping kit only.</description>
                          <conditionFlags><flag name="version">lite</flag></conditionFlags>
                          <files><file source="lite/Campfire.esm" destination="Campfire.esm"/></files>
                          <typeDescriptor><type name="Optional"/></typeDescriptor>
                        </plugin>
                      </plugins>
                    </group>
                    <group name="Extras" type="SelectAny">
                      <plugins>
                        <plugin name="Placeable tents">
                          <description>Adds tents you can drop into the world.</description>
                          <files><file source="tents/Tents.esp" destination="Tents.esp"/></files>
                          <typeDescriptor><type name="Recommended"/></typeDescriptor>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
                <installStep name="Patches">
                  <visible><flagDependency flag="version" value="full"/></visible>
                  <optionalFileGroups>
                    <group name="Frostfall" type="SelectExactlyOne">
                      <plugins>
                        <plugin name="Include patch">
                          <description>Use this when Frostfall is installed.</description>
                          <files><file source="patch/FrostfallPatch.esp" destination="FrostfallPatch.esp"/></files>
                          <typeDescriptor>
                            <dependencyType>
                              <defaultType name="Optional"/>
                              <patterns>
                                <pattern>
                                  <dependencies>
                                    <fileDependency file="Frostfall.esp" state="Missing"/>
                                  </dependencies>
                                  <type name="NotUsable"/>
                                </pattern>
                                <pattern>
                                  <dependencies>
                                    <fileDependency file="Frostfall.esp" state="Active"/>
                                  </dependencies>
                                  <type name="Recommended"/>
                                </pattern>
                              </patterns>
                            </dependencyType>
                          </typeDescriptor>
                        </plugin>
                        <plugin name="Skip patch">
                          <description>Leave Frostfall untouched.</description>
                          <typeDescriptor><type name="Optional"/></typeDescriptor>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
              <conditionalFileInstalls>
                <patterns>
                  <pattern>
                    <dependencies>
                      <flagDependency flag="version" value="full"/>
                    </dependencies>
                    <files><file source="patch/extra.txt" destination="extra.txt"/></files>
                  </pattern>
                </patterns>
              </conditionalFileInstalls>
            </config>
        "#
    }

    #[test]
    fn recommended_option_is_installed_and_the_other_is_not() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        fs::create_dir_all(package.join("full")).unwrap();
        fs::create_dir_all(package.join("lite")).unwrap();
        fs::write(package.join("readme.txt"), b"hello").unwrap();
        fs::write(package.join("full/Campfire.esm"), b"full").unwrap();
        fs::write(package.join("lite/Campfire.esm"), b"lite").unwrap();
        let xml = r#"
            <config>
              <moduleName>Campfire</moduleName>
              <requiredInstallFiles>
                <file source="readme.txt" destination="docs/readme.txt"/>
              </requiredInstallFiles>
              <installSteps>
                <installStep name="Main">
                  <optionalFileGroups>
                    <group name="Core" type="SelectExactlyOne">
                      <plugins>
                        <plugin name="Full">
                          <files><folder source="full" destination=""/></files>
                          <typeDescriptor><type name="Recommended"/></typeDescriptor>
                        </plugin>
                        <plugin name="Lite">
                          <files><folder source="lite" destination=""/></files>
                          <typeDescriptor><type name="Optional"/></typeDescriptor>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
            </config>
        "#;
        let dest = root.path().join("mod");
        let install = install_fomod(&package, xml, &dest).unwrap();
        assert_eq!(install.name, "Campfire");
        assert_eq!(install.notes.len(), 1);
        assert_eq!(install.notes[0].plugin, "Full");
        assert_eq!(fs::read(dest.join("Campfire.esm")).unwrap(), b"full");
        assert_eq!(fs::read(dest.join("docs/readme.txt")).unwrap(), b"hello");
    }

    #[test]
    fn choices_follow_flags_and_installed_plugins() {
        let root = tempfile::tempdir().unwrap();
        let package = sample_package(root.path());
        let xml = sample_xml();
        let plan = preview_fomod(&package, xml, &InstalledFiles::empty(), &[]).unwrap();
        assert!(plan.needs_choices);
        assert_eq!(
            plan.steps[0].groups[0].plugins[0].description,
            "Camping, crafting, and frostfall support."
        );
        assert!(plan.steps[1].visible);
        let frost = &plan.steps[1].groups[0];
        assert_eq!(frost.plugins[0].kind, "NotUsable");
        assert!(frost.plugins[1].selected);
        let image = plan.steps[0].groups[0].plugins[0].image.clone().unwrap();
        assert!(image.ends_with("shot.png"), "{image}");

        let dest = root.path().join("default");
        install_fomod(&package, xml, &dest).unwrap();
        assert_eq!(fs::read(dest.join("Campfire.esm")).unwrap(), b"full");
        assert_eq!(fs::read(dest.join("Tents.esp")).unwrap(), b"tents");
        assert_eq!(fs::read(dest.join("extra.txt")).unwrap(), b"extra");
        assert!(!dest.join("FrostfallPatch.esp").exists());

        let lite = [FomodChoice {
            step: "Options".into(),
            group: "Version".into(),
            plugins: vec!["Lite".into()],
        }];
        let chosen = preview_fomod(&package, xml, &InstalledFiles::empty(), &lite).unwrap();
        assert!(!chosen.steps[1].visible);
        let lite_dest = root.path().join("lite");
        install_fomod_with(&package, xml, &lite_dest, &InstalledFiles::empty(), &lite).unwrap();
        assert_eq!(fs::read(lite_dest.join("Campfire.esm")).unwrap(), b"lite");
        assert!(!lite_dest.join("extra.txt").exists());
        assert!(!lite_dest.join("FrostfallPatch.esp").exists());

        let data = root.path().join("Data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("Frostfall.esp"), b"ff").unwrap();
        let installed = InstalledFiles::new(&data, vec!["Frostfall.esp".into()]);
        let active = preview_fomod(&package, xml, &installed, &[]).unwrap();
        let include = &active.steps[1].groups[0].plugins[0];
        assert_eq!(include.kind, "Recommended");
        assert!(include.selected);
        let patched = root.path().join("patched");
        install_fomod_with(&package, xml, &patched, &installed, &[]).unwrap();
        assert_eq!(
            fs::read(patched.join("FrostfallPatch.esp")).unwrap(),
            b"patch"
        );
    }

    #[test]
    fn an_empty_exact_choice_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let package = sample_package(root.path());
        let choices = [FomodChoice {
            step: "Options".into(),
            group: "Version".into(),
            plugins: Vec::new(),
        }];
        let plan =
            preview_fomod(&package, sample_xml(), &InstalledFiles::empty(), &choices).unwrap();
        assert!(!plan.steps[0].valid);
        let err = install_fomod_with(
            &package,
            sample_xml(),
            &root.path().join("nope"),
            &InstalledFiles::empty(),
            &choices,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Pick one"), "{err}");
        assert!(!root.path().join("nope").exists());
    }

    #[test]
    fn select_all_does_not_need_a_choice() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("a.txt"), b"a").unwrap();
        let xml = r#"
            <config>
              <moduleName>Simple</moduleName>
              <installSteps>
                <installStep name="Main">
                  <optionalFileGroups>
                    <group name="Core" type="SelectAll">
                      <plugins>
                        <plugin name="Files">
                          <files><file source="a.txt" destination="a.txt"/></files>
                          <typeDescriptor><type name="Required"/></typeDescriptor>
                        </plugin>
                      </plugins>
                    </group>
                  </optionalFileGroups>
                </installStep>
              </installSteps>
            </config>
        "#;
        let plan = preview_fomod(&package, xml, &InstalledFiles::empty(), &[]).unwrap();
        assert!(!plan.needs_choices);
        assert!(plan.steps[0].groups[0].plugins[0].locked);
    }
}
