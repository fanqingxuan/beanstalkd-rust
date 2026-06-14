# beanstalkd-rs

`beanstalkd-rs` 是原 C 语言 beanstalkd 服务端的 Rust 重写版本。Rust 实现在
`src/` 中，构建出的二进制仍然叫 `beanstalkd`。

目标是运行时兼容：已有 beanstalkd 客户端应当可以在不修改业务代码的情况下，从 C
版服务迁移到 Rust 版服务。

英文文档见 [README.md](README.md)。

## 当前状态

已实现：

- 基于 TCP 的 beanstalk ASCII 协议。
- 通过 `-l unix:/path/to/socket` 使用 Unix socket。
- 通过 `LISTEN_PID` / `LISTEN_FDS` 支持 systemd socket activation。
- 与 C 版一致的启动顺序：先绑定监听 socket，再按 `-u` 降权。
- `SIGUSR1` drain mode，新 `put` 请求会返回 `DRAINING`。
- 读取 C 版二进制 binlog 的迁移能力，支持当前 WAL v7 和旧 v5。
- Rust 版 JSONL WAL，用于迁移后 Rust 服务写入的新数据。
- 使用真实 Python 客户端库 `greenstalk` 的兼容性测试。

已知差异：

- Rust 服务能读取 C 版 `binlog.N` 文件，但迁移后的新写入会记录到
  `rust-beanstalkd.wal`，不会继续生成新的 C 二进制 binlog 段。

详细兼容性审计见 [COMPATIBILITY_AUDIT.md](COMPATIBILITY_AUDIT.md)。

## 支持的协议命令

生产者和消费者命令：

```text
put
use
reserve
reserve-with-timeout
reserve-job
delete
release
bury
touch
```

tube、查看、统计和控制命令：

```text
watch
ignore
peek
peek-ready
peek-delayed
peek-buried
kick
kick-job
stats
stats-job
stats-tube
list-tubes
list-tube-used
list-tubes-watched
pause-tube
quit
```

## 构建

```sh
cargo build --release
```

构建出的二进制位置：

```sh
target/release/beanstalkd
```

构建独立命令行客户端：

```sh
cargo build --release -p beanstalkctl
```

客户端二进制位置：

```sh
target/release/beanstalkctl
```

如果本机 `cargo` 和 `rustc` 来自不同 toolchain，可以显式指定 Rust 编译器：

```sh
RUSTC=$(rustup which rustc) cargo build --release
```

## 运行

使用默认 TCP 端口：

```sh
target/release/beanstalkd
```

绑定指定地址和端口：

```sh
target/release/beanstalkd -l 127.0.0.1 -p 11300
```

使用 Unix socket：

```sh
target/release/beanstalkd -l unix:/tmp/beanstalkd.sock
```

启用 WAL 持久化：

```sh
target/release/beanstalkd -b /var/lib/beanstalkd
```

## 命令行客户端

仓库中还包含一个独立命令行客户端 `beanstalkctl`，位于 `cli/`。它不依赖服务端
`src/` 中的任何实现，只通过公开 TCP 或 Unix socket 协议操作 beanstalkd。

完整 CLI 文档：

- [cli/README.md](cli/README.md)
- [cli/README.zh-CN.md](cli/README.zh-CN.md)

示例：

```sh
# 向 default tube 写入 job。
target/release/beanstalkctl put "hello"

# 向指定 tube 写入 job。
target/release/beanstalkctl put --tube emails --pri 100 --ttr 60 "send welcome email"

# 只 watch 指定 tube 并 reserve。
target/release/beanstalkctl reserve --watch emails --timeout 5

# reserve 后自动 delete。
target/release/beanstalkctl reserve --timeout 5 --delete

# 删除 job。
target/release/beanstalkctl delete 1

# 查看 job 内容但不消费。
target/release/beanstalkctl peek 1

# 查看 stats。
target/release/beanstalkctl stats
target/release/beanstalkctl stats tube default
target/release/beanstalkctl stats job 1

# 指定服务地址或 Unix socket。
target/release/beanstalkctl --addr 127.0.0.1:11300 tubes
target/release/beanstalkctl --unix /tmp/beanstalkd.sock tubes

# 发送原始协议命令。
target/release/beanstalkctl raw stats

# 启动交互模式。
target/release/beanstalkctl
```

交互模式支持行编辑、历史，以及类似 redis-cli 的完整命令/选项 Tab 补全。详情见
[cli/README.zh-CN.md](cli/README.zh-CN.md)。

## 命令行参数

Rust 版接受与 C 版一致的主要参数：

```text
-b DIR    write-ahead log 目录
-f MS     最多每 MS 毫秒 fsync 一次；-f0 表示总是 fsync
-F        从不 fsync
-l ADDR   监听地址；也可用 unix:/path 指定 Unix socket
-p PORT   监听端口
-s BYTES  WAL 段大小参数，为兼容 C 版而接受
-u USER   绑定监听 socket 后切换到指定用户和用户组
-z BYTES  最大 job 大小
-v        显示版本
-V        增加日志详细程度
-h        显示帮助
```

C 版已移除的 `-c` 和 `-n` 参数也会被接受，并输出兼容性警告。

## 从 C 版 beanstalkd 迁移

1. 平滑停止 C 版 daemon。
2. 保留原 WAL 目录，里面通常包含 `binlog.N` 文件。
3. 使用相同的 `-b DIR` 启动 Rust 版 daemon。
4. 客户端连接配置保持不变。

示例：

```sh
target/release/beanstalkd -l 0.0.0.0 -p 11300 -b /var/lib/beanstalkd
```

启动时，Rust 版会先 replay C 版 `binlog.N`，然后再 replay 已存在的
`rust-beanstalkd.wal`。如果 C 版停止时存在 reserved job，恢复时会按 C 版行为将其
恢复为 ready 状态。

## systemd Socket Activation

Rust 版支持通过 `LISTEN_PID` 和 `LISTEN_FDS` 继承监听 socket。如果存在合法的 TCP
或 Unix listening socket，服务会使用 fd `3`，并忽略手动 `-l` / `-p` 绑定行为，
与 C 版实现一致。

已有 C 版 systemd unit/socket 文件通常可以继续使用，只需要把执行的二进制替换成
Rust 版 `beanstalkd`。

## 测试

安装 Python 测试依赖：

```sh
python -m pip install -r requirements-dev.txt
```

运行 Rust 检查：

```sh
RUSTC=$(rustup which rustc) cargo check
RUSTC=$(rustup which rustc) cargo test
RUSTC=$(rustup which rustc) cargo build --release --workspace
```

运行 Python 客户端兼容性测试：

```sh
python -m pytest -q
```

Python 测试会以子进程启动 Rust daemon，并使用真实 `greenstalk` 客户端库访问服务。
测试覆盖常规客户端操作、tube 行为、delay、bury/kick、pause、drain mode、Rust WAL
恢复、C binlog 迁移，以及 systemd 风格的 fd 继承。

## 目录结构

```text
src/                    Rust 实现
cli/                    独立 beanstalkctl 命令行客户端
tests/                  Python 兼容性测试
COMPATIBILITY_AUDIT.md  兼容性覆盖范围和已知差异
requirements-dev.txt    Python 测试依赖
```

## License

本 Rust 重写版本作为 beanstalkd 的兼容实现维护。
