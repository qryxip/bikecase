use bikecase::{Bikecase, Context};

use structopt::StructOpt as _;

fn main() {
    let opt = Bikecase::from_args();
    let color = opt.color;
    if let Err(err) = Context::new().and_then(|ctx| bikecase::bikecase(opt, ctx)) {
        bikecase::exit_with_error(err, color);
    }
}
