//! Regenerate bundled syntax acknowledgements after updating two-face.
fn main() {
    println!(
        "# Bundled syntax definitions\n\nSource: {}\n",
        two_face::acknowledgement::url()
    );
    let mut text = String::new();
    for license in two_face::acknowledgement::listing().for_syntaxes() {
        license.write_md(&mut text);
    }
    let text = text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    println!("{}", text.trim_end());
}
