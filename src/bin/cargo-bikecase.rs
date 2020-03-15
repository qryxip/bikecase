use bikecase::{Cargo, Context};

use structopt::StructOpt as _;

fn main() -> anyhow::Result<()> {
    let Cargo::Bikecase(opt) = Cargo::from_args();
    bikecase::cargo_bikecase(opt, Context::new()?)
}
