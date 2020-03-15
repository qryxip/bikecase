use bikecase::{Bikecase, Context};

use structopt::StructOpt as _;

fn main() -> anyhow::Result<()> {
    bikecase::bikecase(Bikecase::from_args(), Context::new()?)
}
