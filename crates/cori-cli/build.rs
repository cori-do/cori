// SPDX-License-Identifier: Apache-2.0
// `include_dir!` embeds the templates and the React scaffold (src/init.rs) when the crate compiles; Cargo would not see a
// file added to them, nor always one changed. A directory here is scanned whole.
fn main() {
    println!("cargo:rerun-if-changed=../../templates");
    println!("cargo:rerun-if-changed=front-template");
    println!("cargo:rerun-if-changed=../../packages/client");
}
