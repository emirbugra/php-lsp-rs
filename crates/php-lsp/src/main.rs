use lsp_server::Connection;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// `mi_option_purge_delay` in mimalloc v2/v3 (`libmimalloc_sys` has no named constant for it).
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();

    // give freed pages back to the OS immediately
    unsafe { libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 0) };

    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("php-lsp {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    log::info!("php-lsp {} starting", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();
    let handle = std::thread::Builder::new()
        .name("php-lsp-main".into())
        .stack_size(128 << 20)
        .spawn(move || php_lsp::run(connection))
        .expect("spawn main thread");
    let result = handle.join();
    let _ = io_threads.join();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::error!("server error: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            log::error!("server thread panicked");
            std::process::exit(1);
        }
    }
}
