use crate::istr::IStr;
use crate::origin::{Origin, Origins};
use crate::symbols::{Symbol, SymbolKind};
use crate::types::{Bindings, Types};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymLoc {
    pub file: u32,
    pub idx: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemberKind {
    Method,
    Property,
    Const,
}

impl MemberKind {
    fn matches(self, s: &Symbol, name: &str) -> bool {
        match self {
            MemberKind::Method => s.kind == SymbolKind::Method && s.name.eq_ignore_ascii_case(name),
            MemberKind::Property => s.kind == SymbolKind::Property && s.name == name,
            MemberKind::Const => matches!(s.kind, SymbolKind::ClassConst | SymbolKind::EnumCase) && s.name == name,
        }
    }
}

pub const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";
pub const ELOQUENT_BUILDER: &str = "Illuminate\\Database\\Eloquent\\Builder";
pub const QUERY_BUILDER: &str = "Illuminate\\Database\\Query\\Builder";
pub const ELOQUENT_COLLECTION: &str = "Illuminate\\Database\\Eloquent\\Collection";

/// `Foo<Bar>` -> `Foo` (arrays such as `Foo[]` are returned unchanged and never match a class)
pub fn strip_generic(s: &str) -> &str {
    crate::types::atom_base(s)
}

/// `Foo<A,B>` -> `Some("B")` (the value argument); `mixed` counts as absent.
pub fn generic_arg(s: &str) -> Option<&str> {
    crate::types::generic_args(s).last().copied().filter(|a| *a != "mixed" && !a.is_empty())
}

/// Key-less symbol table: 64-bit FNV-1a of the (optionally lowercased) name -> locations.
/// A 64-bit collision among a few hundred thousand names is negligible (~1e-9).
#[derive(Default)]
struct NameTable {
    map: FxHashMap<u64, Matches>,
}

enum Matches {
    One(SymLoc),
    Many(Vec<SymLoc>),
}

impl Matches {
    fn as_slice(&self) -> &[SymLoc] {
        match self {
            Matches::One(l) => std::slice::from_ref(l),
            Matches::Many(v) => v,
        }
    }
}

fn name_key(name: &str, lower: bool) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in name.as_bytes() {
        let b = if lower { b.to_ascii_lowercase() } else { b };
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl NameTable {
    fn get(&self, name: &str, lower: bool) -> &[SymLoc] {
        self.map.get(&name_key(name, lower)).map(|l| l.as_slice()).unwrap_or(&[])
    }

    fn insert(&mut self, name: &str, lower: bool, loc: SymLoc) -> &mut [SymLoc] {
        use std::collections::hash_map::Entry;
        match self.map.entry(name_key(name, lower)) {
            Entry::Vacant(e) => match e.insert(Matches::One(loc)) {
                Matches::One(l) => std::slice::from_mut(l),
                Matches::Many(_) => unreachable!(),
            },
            Entry::Occupied(e) => {
                let slot = e.into_mut();
                if let Matches::One(first) = slot {
                    *slot = Matches::Many(vec![*first]);
                }
                match slot {
                    Matches::Many(v) => {
                        v.push(loc);
                        v.as_mut_slice()
                    }
                    Matches::One(_) => unreachable!(),
                }
            }
        }
    }

    fn remove(&mut self, name: &str, lower: bool, file: u32, idx: u32) {
        let key = name_key(name, lower);
        let Some(slot) = self.map.get_mut(&key) else { return };
        let empty = match slot {
            Matches::One(l) => l.file == file && l.idx == idx,
            Matches::Many(v) => {
                v.retain(|l| !(l.file == file && l.idx == idx));
                v.is_empty()
            }
        };
        if empty {
            self.map.remove(&key);
        }
    }

    fn all(&self) -> impl Iterator<Item = SymLoc> + '_ {
        self.map.values().flat_map(|l| l.as_slice().iter().copied())
    }
}

pub struct FileData {
    pub uri: IStr,
    pub symbols: Vec<Symbol>,
    /// Sorted hashes of every (short, lowercase) name this file declares or mentions.
    names: Box<[u32]>,
    /// (lowercase container FQN, indices into `symbols`); a file rarely declares more than a few classes.
    members: Box<[(IStr, Box<[u32]>)]>,
}

impl FileData {
    pub fn has_name(&self, hash: u32) -> bool {
        self.names.binary_search(&hash).is_ok()
    }

    pub fn new(uri: String, symbols: Vec<Symbol>, names: Vec<u32>) -> Self {
        let mut members: Vec<(IStr, Vec<u32>)> = Vec::new();
        for (i, s) in symbols.iter().enumerate() {
            if let Some(c) = &s.container {
                let key = IStr::new(&c.to_ascii_lowercase());
                match members.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, v)) => v.push(i as u32),
                    None => members.push((key, vec![i as u32])),
                }
            }
        }
        let members = members.into_iter().map(|(k, v)| (k, v.into_boxed_slice())).collect();
        FileData { uri: IStr::new(&uri), symbols, names: names.into_boxed_slice(), members }
    }

    /// Re-allocate all heap data contiguously. Data built on worker threads sits on pages shared
    /// with short-lived parser garbage; copying it on the owning thread lets those pages be freed.
    pub fn compact(&self) -> FileData {
        FileData {
            uri: self.uri,
            symbols: self.symbols.clone(),
            names: self.names.clone(),
            members: self.members.iter().map(|(k, v)| (*k, v.clone())).collect(),
        }
    }

    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    pub fn members_of(&self, container_lower: &str) -> &[u32] {
        self.members.iter().find(|(k, _)| &**k == container_lower).map(|(_, v)| &v[..]).unwrap_or(&[])
    }
}

#[derive(Default)]
pub struct Index {
    files: Vec<Option<Arc<FileData>>>,
    by_uri: FxHashMap<IStr, u32>,
    free: Vec<u32>,
    classes: NameTable,
    functions: NameTable,
    consts: NameTable,
    origins: Origins,
    /// Global class aliases registered at runtime (Laravel's `Route` → `Illuminate\Support\Facades\Route`),
    /// by lowercase alias. Used only for names no class declares.
    aliases: FxHashMap<String, String>,
    /// Bumped on every change, so results computed from the index can tell they are outdated.
    generation: u64,
}

/// The lookup tables of the index; members live inside their class and are not in any table.
#[derive(Clone, Copy)]
enum Table {
    Classes,
    Functions,
    Consts,
}

/// Table of a symbol, and whether its name is looked up case-insensitively.
fn table_of(kind: SymbolKind) -> Option<(Table, bool)> {
    match kind {
        k if k.is_class_like() => Some((Table::Classes, true)),
        SymbolKind::Function => Some((Table::Functions, true)),
        SymbolKind::Const => Some((Table::Consts, false)),
        _ => None,
    }
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// How files are classified as project / stub / vendor code. Set before indexing.
    pub fn set_origins(&mut self, origins: Origins) {
        self.origins = origins;
    }

    pub fn origins(&self) -> &Origins {
        &self.origins
    }

    /// Stub or vendor file (see `Origins::is_library`).
    pub fn is_library(&self, uri: &str) -> bool {
        self.origins.is_library(uri)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn table(&mut self, t: Table) -> &mut NameTable {
        match t {
            Table::Classes => &mut self.classes,
            Table::Functions => &mut self.functions,
            Table::Consts => &mut self.consts,
        }
    }

    pub fn file_count(&self) -> usize {
        self.by_uri.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.files.iter().flatten().map(|f| f.symbols.len()).sum()
    }

    pub fn has_file(&self, uri: &str) -> bool {
        self.by_uri.contains_key(uri)
    }

    pub fn file(&self, id: u32) -> Option<&Arc<FileData>> {
        self.files.get(id as usize).and_then(|f| f.as_ref())
    }

    pub fn file_by_uri(&self, uri: &str) -> Option<&Arc<FileData>> {
        self.by_uri.get(uri).and_then(|id| self.file(*id))
    }

    pub fn sym(&self, loc: SymLoc) -> &Symbol {
        &self.files[loc.file as usize].as_ref().expect("stale SymLoc").symbols[loc.idx as usize]
    }

    pub fn uri_of(&self, loc: SymLoc) -> &str {
        &self.files[loc.file as usize].as_ref().expect("stale SymLoc").uri
    }

    pub fn remove_file(&mut self, uri: &str) {
        let Some(id) = self.by_uri.remove(uri) else { return };
        let Some(old) = self.files[id as usize].take() else { return };
        self.generation += 1;
        for (i, s) in old.symbols.iter().enumerate() {
            let Some((table, lower)) = table_of(s.kind) else { continue };
            self.table(table).remove(&s.fqn, lower, id, i as u32);
        }
        self.free.push(id);
    }

    pub fn update_file(&mut self, data: FileData) {
        self.remove_file(&data.uri);
        let id = match self.free.pop() {
            Some(id) => id,
            None => {
                self.files.push(None);
                (self.files.len() - 1) as u32
            }
        };
        self.generation += 1;
        for (i, s) in data.symbols.iter().enumerate() {
            let Some((table, lower)) = table_of(s.kind) else { continue };
            let map = match table {
                Table::Classes => &mut self.classes,
                Table::Functions => &mut self.functions,
                Table::Consts => &mut self.consts,
            };
            let v = map.insert(&s.fqn, lower, SymLoc { file: id, idx: i as u32 });
            if v.len() > 1 {
                // project code first, then built-in stubs, then vendor/ (stable sort keeps load order otherwise)
                let (files, origins) = (&self.files, &self.origins);
                let origin = |l: &SymLoc| -> Origin {
                    let uri: &str = if l.file == id {
                        &data.uri
                    } else {
                        files[l.file as usize].as_ref().map(|f| &*f.uri).unwrap_or("")
                    };
                    origins.of(uri)
                };
                v.sort_by_key(origin);
            }
        }
        self.by_uri.insert(data.uri, id);
        self.files[id as usize] = Some(Arc::new(data));
    }

    pub fn file_id(&self, uri: &str) -> Option<u32> {
        self.by_uri.get(uri).copied()
    }

    /// Ids of all files that declare or mention `hash` (see `refnames::name_hash`).
    pub fn files_with_name(&self, hash: u32) -> Vec<u32> {
        self.files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.as_ref().filter(|f| f.has_name(hash)).map(|_| i as u32))
            .collect()
    }

    pub fn find_classes(&self, fqn: &str) -> &[SymLoc] {
        let name = strip_generic(fqn).trim_start_matches('\\');
        let found = self.classes.get(name, true);
        if found.is_empty() && !self.aliases.is_empty() && !name.contains('\\') {
            if let Some(target) = self.aliases.get(&name.to_ascii_lowercase()) {
                return self.classes.get(target, true);
            }
        }
        found
    }

    /// Set the global class aliases (`alias` → target FQN).
    pub fn set_class_aliases(&mut self, aliases: impl IntoIterator<Item = (String, String)>) {
        self.aliases = aliases.into_iter().map(|(a, t)| (a.to_ascii_lowercase(), t)).collect();
        self.generation += 1;
    }

    pub fn find_class(&self, fqn: &str) -> Option<SymLoc> {
        self.find_classes(fqn).first().copied()
    }

    pub fn find_function(&self, fqn: &str) -> Option<SymLoc> {
        self.find_functions(fqn).first().copied()
    }

    /// Every definition of a function (`if (!function_exists('x'))` guards allow several).
    pub fn find_functions(&self, fqn: &str) -> &[SymLoc] {
        self.functions.get(fqn.trim_start_matches('\\'), true)
    }

    /// Every definition of a constant.
    pub fn find_consts(&self, fqn: &str) -> &[SymLoc] {
        self.consts.get(fqn.trim_start_matches('\\'), false)
    }

    pub fn find_const(&self, fqn: &str) -> Option<SymLoc> {
        self.consts.get(fqn.trim_start_matches('\\'), false).first().copied()
    }

    /// Class, its traits, parent chain, then interfaces. Each class once.
    pub fn ancestors(&self, fqn: &str) -> Vec<SymLoc> {
        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        self.collect_ancestors(fqn, &mut out, &mut seen, 0);
        out
    }

    fn collect_ancestors(&self, fqn: &str, out: &mut Vec<SymLoc>, seen: &mut FxHashSet<String>, depth: u32) {
        if depth > 64 {
            return;
        }
        let fqn = strip_generic(fqn);
        let key = fqn.trim_start_matches('\\').to_ascii_lowercase();
        if !seen.insert(key) {
            return;
        }
        let Some(loc) = self.find_class(fqn) else { return };
        out.push(loc);
        let s = self.sym(loc);
        for t in s.traits() {
            self.collect_ancestors(t, out, seen, depth + 1);
        }
        for e in s.extends() {
            self.collect_ancestors(e, out, seen, depth + 1);
        }
        for i in s.implements() {
            self.collect_ancestors(i, out, seen, depth + 1);
        }
    }

    /// Template bindings of `container` as seen from the receiver atom `recv`, following
    /// `@extends` / `@implements` / `@use` instantiations up the hierarchy.
    pub fn class_bindings(&self, receiver: &str, container: &str) -> Bindings {
        let mut seen = FxHashSet::default();
        self.walk_bindings(receiver, container, 0, &mut seen).unwrap_or_default()
    }

    fn own_bindings(&self, sym: &Symbol, atom: &str) -> Bindings {
        let mut b = Bindings::default();
        let args = crate::types::generic_args(atom);
        for (i, (name, _)) in sym.templates().iter().enumerate() {
            if let Some(a) = args.get(i) {
                b.bind(name, crate::types::arg_atoms(a));
            }
        }
        b
    }

    fn walk_bindings(&self, atom: &str, target: &str, depth: u32, seen: &mut FxHashSet<String>) -> Option<Bindings> {
        let base = strip_generic(atom);
        if depth > 24 || !seen.insert(atom.to_ascii_lowercase()) {
            return None;
        }
        let loc = self.find_class(base)?;
        let sym = self.sym(loc);
        let own = self.own_bindings(sym, atom);
        if base.trim_start_matches('\\').eq_ignore_ascii_case(target.trim_start_matches('\\')) {
            return Some(own);
        }
        let mut next: Vec<String> = sym.inherit().iter().map(|i| i.to_string()).collect();
        let covered: Vec<String> = next.iter().map(|n| strip_generic(n).to_ascii_lowercase()).collect();
        for n in sym.extends().iter().chain(sym.implements()).chain(sym.traits()) {
            if !covered.contains(&n.to_ascii_lowercase()) {
                next.push(n.to_string());
            }
        }
        for n in next {
            let n = crate::types::subst(&[n], &own).pop().unwrap_or_default();
            if n.is_empty() {
                continue;
            }
            if let Some(b) = self.walk_bindings(&n, target, depth + 1, seen) {
                return Some(b);
            }
        }
        None
    }

    /// Element atoms when iterating or indexing a non-array atom (`Collection<int,User>` -> `User`).
    pub fn iter_elems(&self, atom: &str) -> Types {
        let args = crate::types::generic_args(atom);
        let base = strip_generic(atom);
        let pick = if base.eq_ignore_ascii_case("Generator") { args.get(1).copied() } else { args.last().copied() };
        match pick {
            Some(a) => crate::types::arg_atoms(a),
            None => {
                let _ = base;
                Vec::new()
            }
        }
    }

    /// Every class, interface and trait reachable from `fqn` is present in the index.
    pub fn hierarchy_complete(&self, fqn: &str) -> bool {
        let mut seen = FxHashSet::default();
        self.hierarchy_complete_inner(fqn, &mut seen, 0)
    }

    fn hierarchy_complete_inner(&self, fqn: &str, seen: &mut FxHashSet<String>, depth: u32) -> bool {
        if depth > 64 {
            return false;
        }
        let fqn = strip_generic(fqn);
        if !seen.insert(fqn.trim_start_matches('\\').to_ascii_lowercase()) {
            return true;
        }
        let Some(loc) = self.find_class(fqn) else { return false };
        let s = self.sym(loc);
        s.extends()
            .iter()
            .chain(s.implements().iter())
            .chain(s.traits().iter())
            .all(|n| self.hierarchy_complete_inner(n, seen, depth + 1))
    }

    /// Does `fqn` or an ancestor forward unknown members (`__call`, `__callStatic`, `@mixin`)?
    pub fn has_dynamic_methods(&self, fqn: &str) -> bool {
        let fqn = strip_generic(fqn);
        self.ancestors(fqn).iter().any(|l| self.sym(*l).mixin)
            || self.find_member_direct(fqn, "__call", MemberKind::Method).is_some()
            || self.find_member_direct(fqn, "__callStatic", MemberKind::Method).is_some()
    }

    /// Abstract methods (from abstract classes / interfaces / traits) that `fqn` does not implement.
    pub fn missing_abstract(&self, fqn: &str) -> Vec<SymLoc> {
        let mut concrete: FxHashSet<String> = FxHashSet::default();
        let mut abstracts: Vec<SymLoc> = Vec::new();
        for cls in self.ancestors(fqn) {
            let Some(file) = self.file(cls.file) else { continue };
            let key = self.sym(cls).fqn.to_ascii_lowercase();
            for &i in file.members_of(&key) {
                let s = &file.symbols[i as usize];
                if s.kind != SymbolKind::Method {
                    continue;
                }
                if s.is_abstract && !s.is_virtual {
                    abstracts.push(SymLoc { file: cls.file, idx: i });
                } else {
                    concrete.insert(s.name.to_ascii_lowercase());
                }
            }
        }
        let mut seen = FxHashSet::default();
        abstracts
            .into_iter()
            .filter(|l| {
                let n = self.sym(*l).name.to_ascii_lowercase();
                !concrete.contains(&n) && seen.insert(n)
            })
            .collect()
    }

    pub fn is_subclass_of(&self, fqn: &str, ancestor: &str) -> bool {
        let a = ancestor.trim_start_matches('\\');
        self.ancestors(fqn).iter().any(|l| self.sym(*l).fqn.eq_ignore_ascii_case(a))
    }

    /// Is `fqn` an Eloquent model (or the base model itself)?
    pub fn is_model(&self, fqn: &str) -> bool {
        let fqn = strip_generic(fqn);
        fqn.trim_start_matches('\\').eq_ignore_ascii_case(MODEL) || self.is_subclass_of(fqn, MODEL)
    }

    /// Member lookup including inheritance and Laravel Eloquent forwarding
    /// (`Model::where()` -> `Eloquent\Builder`, `Builder<M>::scopeName()` -> `M`).
    pub fn find_member(&self, class_fqn: &str, name: &str, kind: MemberKind) -> Option<SymLoc> {
        let base = strip_generic(class_fqn);
        if let Some(l) = self.find_member_direct(base, name, kind) {
            return Some(l);
        }
        if kind != MemberKind::Method || name.starts_with("__") {
            return None;
        }
        let is_builder = base.eq_ignore_ascii_case(ELOQUENT_BUILDER) || base.eq_ignore_ascii_case(QUERY_BUILDER);
        if is_builder {
            // scopes and model methods called on a query builder bound to a model
            if let Some(m) = generic_arg(class_fqn) {
                if let Some(l) = self.find_member_direct(strip_generic(m), name, kind) {
                    return Some(l);
                }
            }
            // Eloquent\Builder::__call forwards to Query\Builder
            if base.eq_ignore_ascii_case(ELOQUENT_BUILDER) {
                if let Some(l) = self.find_member_direct(QUERY_BUILDER, name, kind) {
                    if self.sym(l).vis == php_syntax::Visibility::Public {
                        return Some(l);
                    }
                }
            }
            return None;
        }
        if self.is_model(base) {
            for b in [ELOQUENT_BUILDER, QUERY_BUILDER] {
                if let Some(l) = self.find_member_direct(b, name, kind) {
                    let s = self.sym(l);
                    if s.vis == php_syntax::Visibility::Public && !s.is_static {
                        return Some(l);
                    }
                }
            }
        }
        None
    }

    pub fn find_member_direct(&self, class_fqn: &str, name: &str, kind: MemberKind) -> Option<SymLoc> {
        for cls in self.ancestors(class_fqn) {
            let csym = self.sym(cls);
            let file = self.file(cls.file)?;
            let key = csym.fqn.to_ascii_lowercase();
            for &i in file.members_of(&key) {
                let s = &file.symbols[i as usize];
                if kind.matches(s, name) {
                    return Some(SymLoc { file: cls.file, idx: i });
                }
            }
        }
        None
    }

    /// All members visible through inheritance, nearest declaration wins.
    pub fn all_members(&self, class_fqn: &str) -> Vec<SymLoc> {
        let mut out = Vec::new();
        let mut seen: FxHashSet<(u8, String)> = FxHashSet::default();
        for cls in self.ancestors(class_fqn) {
            let csym = self.sym(cls);
            let Some(file) = self.file(cls.file) else { continue };
            let key = csym.fqn.to_ascii_lowercase();
            for &i in file.members_of(&key) {
                let s = &file.symbols[i as usize];
                let (g, n) = match s.kind {
                    SymbolKind::Method => (0u8, s.name.to_ascii_lowercase()),
                    SymbolKind::Property => (1, s.name.to_string()),
                    _ => (2, s.name.to_string()),
                };
                if seen.insert((g, n)) {
                    out.push(SymLoc { file: cls.file, idx: i });
                }
            }
        }
        out
    }

    /// Direct sub-types (extends / implements / trait use) of every class-like, keyed by lowercase FQN.
    pub fn subtype_map(&self) -> FxHashMap<String, Vec<SymLoc>> {
        let mut m: FxHashMap<String, Vec<SymLoc>> = FxHashMap::default();
        for loc in self.all_classes() {
            let s = self.sym(loc);
            for b in s.extends().iter().chain(s.implements()).chain(s.traits()) {
                let k = strip_generic(b).trim_start_matches('\\').to_ascii_lowercase();
                let v = m.entry(k).or_default();
                if !v.contains(&loc) {
                    v.push(loc);
                }
            }
        }
        m
    }

    /// Direct sub-types of `fqn`.
    pub fn subtypes(&self, fqn: &str) -> Vec<SymLoc> {
        let k = strip_generic(fqn).trim_start_matches('\\').to_ascii_lowercase();
        self.subtype_map().remove(&k).unwrap_or_default()
    }

    /// All transitive sub-types of `fqn` (breadth first, each once).
    pub fn descendants(&self, fqn: &str) -> Vec<SymLoc> {
        self.descendants_with(&self.subtype_map(), fqn)
    }

    /// [`Self::descendants`] with a [`Self::subtype_map`] computed once for many queries.
    pub fn descendants_with(&self, map: &FxHashMap<String, Vec<SymLoc>>, fqn: &str) -> Vec<SymLoc> {
        let mut out: Vec<SymLoc> = Vec::new();
        let mut seen: FxHashSet<SymLoc> = FxHashSet::default();
        let mut queue = vec![strip_generic(fqn).trim_start_matches('\\').to_ascii_lowercase()];
        while let Some(k) = queue.pop() {
            for &l in map.get(&k).map(|v| v.as_slice()).unwrap_or(&[]) {
                if seen.insert(l) {
                    out.push(l);
                    queue.push(self.sym(l).fqn.trim_start_matches('\\').to_ascii_lowercase());
                }
            }
        }
        out
    }

    /// Iterate over every class-like symbol.
    pub fn all_classes(&self) -> impl Iterator<Item = SymLoc> + '_ {
        self.classes.all()
    }

    pub fn all_functions(&self) -> impl Iterator<Item = SymLoc> + '_ {
        self.functions.all()
    }

    pub fn all_consts(&self) -> impl Iterator<Item = SymLoc> + '_ {
        self.consts.all()
    }

    /// Fuzzy workspace symbol search over classes, functions and constants.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SymLoc> {
        let q = query;
        let mut scored: Vec<(u32, SymLoc)> = Vec::new();
        let mut consider = |loc: SymLoc, s: &Symbol| {
            if let Some(score) = fuzzy_score_loose(q, &s.name) {
                scored.push((score, loc));
            }
        };
        for loc in self.all_classes().chain(self.all_functions()).chain(self.all_consts()) {
            consider(loc, self.sym(loc));
        }
        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| self.sym(a.1).name.len().cmp(&self.sym(b.1).name.len()))
                .then_with(|| self.sym(a.1).fqn.cmp(&self.sym(b.1).fqn))
        });
        scored.truncate(limit);
        scored.into_iter().map(|(_, l)| l).collect()
    }
}

/// Case-insensitive match score, lower is better:
/// 0 exact, 1 prefix, 2 substring, 3 camel-hump (`ProdCon` -> `ProductsController`).
/// Allocation free: this runs over every symbol on each keystroke.
pub fn fuzzy_score(query: &str, name: &str) -> Option<u32> {
    let q = query.as_bytes();
    let n = name.as_bytes();
    if q.is_empty() {
        return Some(10);
    }
    if q.len() > n.len() {
        return None;
    }
    if n.eq_ignore_ascii_case(q) {
        return Some(0);
    }
    if n[..q.len()].eq_ignore_ascii_case(q) {
        return Some(1);
    }
    if n.windows(q.len()).any(|w| w.eq_ignore_ascii_case(q)) {
        return Some(2);
    }
    if hump_match(q, n) {
        return Some(3);
    }
    None
}

/// Like [`fuzzy_score`] but also accepts a plain subsequence (score 4).
pub fn fuzzy_score_loose(query: &str, name: &str) -> Option<u32> {
    if let Some(s) = fuzzy_score(query, name) {
        return Some(s);
    }
    let q = query.as_bytes();
    let mut qi = 0;
    for &c in name.as_bytes() {
        if c.eq_ignore_ascii_case(&q[qi]) {
            qi += 1;
            if qi == q.len() {
                return Some(4);
            }
        }
    }
    None
}

fn hump_match(q: &[u8], n: &[u8]) -> bool {
    // humps start at index 0, at an uppercase letter, or after `_`
    let mut qi = 0;
    let mut i = 0;
    while i < n.len() && qi < q.len() {
        let is_start = i == 0 || n[i].is_ascii_uppercase() || n[i - 1] == b'_' || n[i - 1] == b'\\';
        if is_start && n[i].eq_ignore_ascii_case(&q[qi]) {
            // consume as much of this hump as the query allows
            while i < n.len() && qi < q.len() && n[i].eq_ignore_ascii_case(&q[qi]) {
                i += 1;
                qi += 1;
                if i < n.len() && n[i].is_ascii_uppercase() {
                    break;
                }
            }
        } else {
            i += 1;
        }
    }
    qi == q.len()
}
