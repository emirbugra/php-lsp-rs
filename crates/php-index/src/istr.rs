//! Global string interner. Index data repeats the same names (class FQNs, types, parameter
//! names) thousands of times; an `IStr` is a deduplicated, immortal `&'static str`.

use rustc_hash::{FxHashSet, FxHasher};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::{Mutex, OnceLock};

const SHARDS: usize = 64;

fn shards() -> &'static [Mutex<FxHashSet<IStr>>] {
    static S: OnceLock<Vec<Mutex<FxHashSet<IStr>>>> = OnceLock::new();
    S.get_or_init(|| (0..SHARDS).map(|_| Mutex::new(FxHashSet::default())).collect())
}

/// Thin pointer to `[u32 len][utf-8 bytes]`, leaked once per distinct string (8 bytes instead of 16).
#[derive(Clone, Copy)]
pub struct IStr(std::ptr::NonNull<u8>);

unsafe impl Send for IStr {}
unsafe impl Sync for IStr {}

static EMPTY: [u8; 4] = [0; 4];

impl IStr {
    fn empty() -> IStr {
        IStr(std::ptr::NonNull::from(&EMPTY).cast())
    }

    pub fn new(s: &str) -> IStr {
        if s.is_empty() {
            return IStr::empty();
        }
        let mut h = FxHasher::default();
        s.hash(&mut h);
        let shard = &shards()[(h.finish() as usize >> 7) % SHARDS];
        let mut set = shard.lock().unwrap();
        if let Some(e) = set.get(s) {
            return *e;
        }
        let mut buf = Vec::with_capacity(4 + s.len());
        buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
        let leaked: &'static mut [u8] = Box::leak(buf.into_boxed_slice());
        let e = IStr(std::ptr::NonNull::from(&mut leaked[0]));
        set.insert(e);
        e
    }

    pub fn as_str(&self) -> &'static str {
        unsafe {
            let p = self.0.as_ptr();
            let len = (p as *const u32).read_unaligned() as usize;
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(p.add(4), len))
        }
    }
}

impl Default for IStr {
    fn default() -> Self {
        IStr::empty()
    }
}

impl Deref for IStr {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for IStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for IStr {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for IStr {
    fn eq(&self, o: &IStr) -> bool {
        self.0 == o.0 || self.as_str() == o.as_str()
    }
}
impl Eq for IStr {}
impl PartialEq<str> for IStr {
    fn eq(&self, o: &str) -> bool {
        self.as_str() == o
    }
}
impl PartialEq<&str> for IStr {
    fn eq(&self, o: &&str) -> bool {
        self.as_str() == *o
    }
}
impl PartialEq<String> for IStr {
    fn eq(&self, o: &String) -> bool {
        self.as_str() == o.as_str()
    }
}
impl Hash for IStr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_str().hash(h)
    }
}
impl PartialOrd for IStr {
    fn partial_cmp(&self, o: &IStr) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for IStr {
    fn cmp(&self, o: &IStr) -> std::cmp::Ordering {
        self.as_str().cmp(o.as_str())
    }
}
impl fmt::Display for IStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl fmt::Debug for IStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}
impl From<&str> for IStr {
    fn from(s: &str) -> IStr {
        IStr::new(s)
    }
}
impl From<String> for IStr {
    fn from(s: String) -> IStr {
        IStr::new(&s)
    }
}
impl From<&String> for IStr {
    fn from(s: &String) -> IStr {
        IStr::new(s)
    }
}
impl From<IStr> for String {
    fn from(s: IStr) -> String {
        s.as_str().to_string()
    }
}

pub fn istrs<S: AsRef<str>>(v: impl IntoIterator<Item = S>) -> Vec<IStr> {
    v.into_iter().map(|s| IStr::new(s.as_ref())).collect()
}

/// (distinct strings, total bytes) held by the interner.
pub fn stats() -> (usize, usize) {
    let mut n = 0;
    let mut b = 0;
    for s in shards() {
        let g = s.lock().unwrap();
        n += g.len();
        b += g.iter().map(|x| x.len()).sum::<usize>();
    }
    (n, b)
}
