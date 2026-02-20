// This program is exported as a binary named `amzn-kiro-assistant`.
//
// You can run it via Brazil:
//
// ```console
// $ brazil-build # needed once
// $ brazil-runtime-exec amzn-kiro-assistant
// ```

use amzn_kiro_assistant::hello;

fn main() {
    println!("{}", hello("Kiro-Assistant"));
}
