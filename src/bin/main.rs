use libvoir::Voir;

fn main() {
    let mut voir = Voir::default();

    voir.open("target/debug/voir");
    voir.hex();
}
