use eyre::Report;

pub fn print(error: &Report, verbose: u8) {
    let mut chain = error.chain();
    let Some(head) = chain.next() else {
        eprintln!("error: {error}");
        return;
    };

    eprintln!("{} {head}", crate::ui::error_label());

    if verbose == 0 {
        if chain.next().is_some() {
            eprintln!("\nhint: rerun with -v for more detail");
        }
        return;
    }

    for (index, cause) in error.chain().skip(1).enumerate() {
        eprintln!("  {}: {cause}", index + 1);
    }
}
