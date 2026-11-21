pub(crate) fn builder() -> env_logger::Builder {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(log::LevelFilter::Info);
    builder.filter_module("mainline", log::LevelFilter::Warn);
    builder
}
