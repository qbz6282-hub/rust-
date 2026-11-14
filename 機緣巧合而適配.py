import os, shutil, subprocess, urllib.request

# ---- 重建目标目录（沙箱每次执行都是干净的，故本轮内完成安装+写码+编译验证）----
TARGET = "/workspace/rust/9178x6769"
shutil.rmtree(TARGET, ignore_errors=True)
os.makedirs(TARGET + "/src", exist_ok=True)
os.makedirs("/tmp/rustup", exist_ok=True)
os.makedirs("/tmp/cargo", exist_ok=True)
INIT = "/tmp/rustup-init"
if not os.path.exists(INIT):
    urllib.request.urlretrieve(
        "https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init", INIT)
    os.chmod(INIT, 0o755)

# ---- 1. Cargo.toml ----
open(TARGET + "/Cargo.toml", "w").write('''[package]
name = "multiply_9178x6769"
version = "0.1.0"
edition = "2021"

[profile.release]
opt-level = 3
''')

# ---- 2. src/main.rs ----
open(TARGET + "/src/main.rs", "w").write('''//! 计算 9178 × 6769

/// 一次乘法运算的两个操作数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Multiply {
    a: i64,
    b: i64,
}

impl Multiply {
    pub fn new(a: i64, b: i64) -> Self {
        Self { a, b }
    }

    /// 乘积。i64 相乘可能溢位，故回传 Option<i64>
    pub fn result(&self) -> Option<i64> {
        self.a.checked_mul(self.b)
    }
}

fn main() {
    let task = Multiply::new(9178, 6769);

    match task.result() {
        Some(v) => println!("{a} * {b} = {v}", a = task.a, b = task.b),
        None => eprintln!("multiply overflow"),
    }
}

#[cfg(test)]
mod tests {
    use super::Multiply;

    #[test]
    fn multiplies_correctly() {
        assert_eq!(Multiply::new(9178, 6769).result(), Some(62_125_882));
    }

    #[test]
    fn is_commutative() {
        assert_eq!(Multiply::new(9178, 6769).result(),
                   Multiply::new(6769, 9178).result());
    }

    #[test]
    fn detects_overflow() {
        assert_eq!(Multiply::new(i64::MAX, 2).result(), None);
    }
}
''')

# ---- 3. 编译并执行，把结果与产物清单写入 /tmp/log.txt ----
env = dict(os.environ, RUSTUP_HOME="/tmp/rustup", CARGO_HOME="/tmp/cargo",
           PATH="/tmp/cargo/bin:" + os.environ.get("PATH", ""))
sh = lambda c, **k: subprocess.run(c, shell=True, capture_output=True, text=True, env=env, **k)

log = []
r = sh(f"{INIT} -y --no-modify-path --profile minimal --default-toolchain stable > /tmp/inst.log 2>&1")
log.append(f"rustup rc={r.returncode}")

CARGO = "/tmp/cargo/bin/cargo"
r = sh(f"{CARGO} --version"); log.append("cargo: " + (r.stdout or r.stderr).strip())
r = sh(f"{CARGO} run --release --quiet", cwd=TARGET)
log.append(f"cargo run rc={r.returncode}\nSTDOUT>>>{r.stdout.strip()}<<<\nSTDERR>>>{r.stderr.strip()[:400]}<<<")
r = sh(f"{CARGO} test --release --quiet", cwd=TARGET)
log.append(f"cargo test rc={r.returncode}\n{r.stdout.strip()[:400]}\n{r.stderr.strip()[:300]}")

files = []
for dp, dn, fn in os.walk(TARGET):
    dn[:] = [d for d in dn if d != "target"]
    for f in fn:
        files.append(os.path.join(dp, f))
log.append("source files: " + repr(files))
open("/tmp/log.txt", "w").write("\n".join(log))
print("DONE", flush=True)
