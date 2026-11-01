use std::time::Instant;
fn main() {
    let root = std::env::args().nth(1).expect("dir");
    let mut files = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().map(|n| n != ".git" && n != "node_modules").unwrap_or(true) {
                    stack.push(p);
                }
            } else if p.extension().map(|x| x == "php").unwrap_or(false) {
                files.push(p);
            }
        }
    }
    let t = Instant::now();
    let (mut bytes, mut errs, mut bad) = (0usize, 0usize, 0usize);
    for f in &files {
        let Ok(s) = std::fs::read_to_string(f) else { continue };
        bytes += s.len();
        let r = php_syntax::parse(&s);
        if !r.errors.is_empty() {
            bad += 1;
            errs += r.errors.len();
            if bad <= 15 {
                let e = &r.errors[0];
                let lines = php_syntax::LineIndex::new(&s);
                let (l, c) = lines.position(&s, e.span.start as usize);
                println!("{}:{}:{}: {}", f.display(), l + 1, c + 1, e.message);
            }
        }
    }
    println!(
        "{} files, {} MB, {} files with errors ({} errors), {:?}",
        files.len(),
        bytes / 1_000_000,
        bad,
        errs,
        t.elapsed()
    );
}
