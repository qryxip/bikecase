use bikecase::{Cargo, Context};

use structopt::StructOpt as _;

fn main() {
    let Cargo::Bikecase(opt) = Cargo::from_args();
    let color = opt.color();
    if let Err(err) = Context::new().and_then(|ctx| bikecase::cargo_bikecase(opt, ctx)) {
        bikecase::exit_with_error(err, color);
    }
}
