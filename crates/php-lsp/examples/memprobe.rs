use php_index::*;
use std::path::Path;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn rss() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("VmRSS"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<usize>()
        .unwrap()
        / 1024
}

fn main() {
    let root = std::env::args().nth(1).unwrap();
    println!(
        "sizeof Symbol={} ParamInfo={} IStr={}",
        std::mem::size_of::<Symbol>(),
        std::mem::size_of::<ParamInfo>(),
        std::mem::size_of::<IStr>()
    );
    let mut files = vec![];
    for e in ignore::WalkBuilder::new(&root).hidden(false).git_ignore(false).build().flatten() {
        if e.path().extension().map(|x| x == "php").unwrap_or(false) {
            files.push(e.path().to_path_buf());
        }
    }
    let base = rss();
    if std::env::args().nth(2).as_deref() == Some("par") {
        use rayon::prelude::*;
        let pool = rayon::ThreadPoolBuilder::new().num_threads(15).build().unwrap();
        let data: Vec<FileData> = pool.install(|| {
            files
                .par_chunks(64)
                .flat_map_iter(|c| {
                    c.iter()
                        .filter_map(|p| {
                            let src = std::fs::read_to_string(p).ok()?;
                            Some(index_source(format!("file://{}", p.display()), &src))
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        });
        drop(pool);
        let data: Vec<FileData> = if std::env::args().nth(3).as_deref() == Some("compact") {
            let n: Vec<FileData> = data.iter().map(|d| d.compact()).collect();
            drop(data);
            n
        } else {
            data
        };
        unsafe { libmimalloc_sys::mi_collect(true) };
        println!("parallel: files={} rss delta={}MB", data.len(), rss() - base);
        return;
    }
    let mut data = Vec::new();
    let (mut syms, mut params, mut names_len, mut src_bytes) = (0usize, 0usize, 0usize, 0usize);
    for p in &files {
        let Ok(src) = std::fs::read_to_string(Path::new(p)) else { continue };
        src_bytes += src.len();
        let d = index_source(format!("file://{}", p.display()), &src);
        syms += d.symbols.len();
        params += d.symbols.iter().map(|s| s.params.len()).sum::<usize>();
        names_len += d.name_count();
        data.push(d);
    }
    unsafe { libmimalloc_sys::mi_collect(true) };
    let after = rss();
    println!(
        "files={} src={}MB symbols={} params={} names={} rss delta={}MB",
        data.len(),
        src_bytes >> 20,
        syms,
        params,
        names_len,
        after - base
    );
    let mut idx = Index::new();
    for d in data {
        idx.update_file(d);
    }
    unsafe { libmimalloc_sys::mi_collect(true) };
    let (n, b) = php_index::istr::stats();
    println!("interned: {} strings, {} KB", n, b >> 10);
    let uri: usize = idx.file_count();
    println!("files in index: {uri}");
    println!("after Index::update_file: rss delta={}MB", rss() - base);
}
