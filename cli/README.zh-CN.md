# beanstalkctl

`beanstalkctl` 是一个独立的 beanstalkd 命令行客户端。它不依赖 Rust 服务端
`src/` 中的任何实现，只通过公开 TCP 或 Unix socket 协议操作 beanstalkd。

English documentation: [README.md](README.md).

## 构建

在仓库根目录执行：

```sh
cargo build --release -p beanstalkctl
```

生成的二进制位置：

```sh
target/release/beanstalkctl
```

## 连接参数

```text
-a, --addr ADDR    TCP host:port、host，或 unix:/path
-H, --host HOST    TCP host，默认 127.0.0.1
-p, --port PORT    TCP port，默认 11300
    --unix PATH    连接 Unix socket
```

示例：

```sh
beanstalkctl --addr 127.0.0.1:11300 tubes
beanstalkctl --host 127.0.0.1 --port 11300 stats
beanstalkctl --unix /tmp/beanstalkd.sock tubes
```

## 命令

不带命令直接运行 `beanstalkctl` 会进入交互模式。

```text
put [OPTIONS] BODY...             插入 job
reserve [--timeout N] [--watch T] [--delete]
delete ID                         删除 job
release ID [PRI] [DELAY]          释放 reserved job
bury ID [PRI]                     bury reserved job
touch ID                          touch reserved job
peek ID                           按 job id 查看
peek-ready                        查看下一个 ready job
peek-delayed                      查看下一个 delayed job
peek-buried                       查看下一个 buried job
kick BOUND                        kick buried 或 delayed job
kick-job ID                       kick 单个 buried 或 delayed job
stats [job ID|tube NAME]          查看 server、job 或 tube stats
tubes                             列出 tubes
using                             查看当前 use 的 tube
watching                          查看当前 watched tubes
pause-tube TUBE DELAY             暂停 tube
raw COMMAND...                    发送原始协议命令
repl | interactive                进入交互模式
```

## 写入 Job

写入简单 job：

```sh
beanstalkctl put "hello"
```

写入指定 tube：

```sh
beanstalkctl put --tube emails --pri 100 --delay 0 --ttr 60 "send welcome email"
```

从文件读取 body：

```sh
beanstalkctl put --tube imports --file payload.json
```

从标准输入读取 body：

```sh
printf 'payload' | beanstalkctl put --stdin
```

## 消费 Job

从默认 watch list reserve：

```sh
beanstalkctl reserve --timeout 5
```

从指定 tube reserve：

```sh
beanstalkctl reserve --watch emails --timeout 5
```

reserve 后立即 delete：

```sh
beanstalkctl reserve --timeout 5 --delete
```

## 管理 Job

```sh
beanstalkctl delete 1
beanstalkctl release 1 65536 0
beanstalkctl bury 1 65536
beanstalkctl touch 1
beanstalkctl kick 10
beanstalkctl kick-job 1
```

## 查看 Job 和 Tube

```sh
beanstalkctl peek 1
beanstalkctl peek-ready
beanstalkctl peek-delayed
beanstalkctl peek-buried
beanstalkctl tubes
beanstalkctl using
beanstalkctl watching
```

Job 响应会以带标签的格式打印：

```text
status: found
job id: 1
bytes: 4
body:
name
```

## Stats

```sh
beanstalkctl stats
beanstalkctl stats job 1
beanstalkctl stats tube default
```

## 原始协议命令

如果某个新命令或少见命令还没有封装，可以使用 `raw`：

```sh
beanstalkctl raw stats
beanstalkctl raw list-tubes
beanstalkctl raw pause-tube default 5
```

## 交互模式

启动交互会话：

```sh
beanstalkctl repl
```

也可以不带命令直接运行：

```sh
beanstalkctl
```

进入后会看到提示符：

```text
beanstalkctl>
```

交互模式里的命令语法与普通 `beanstalkctl` 命令一致：

```text
beanstalkctl> put --tube emails "hello"
INSERTED 1
beanstalkctl> reserve --watch emails --timeout 5 --delete
RESERVED 1 5
hello
DELETED
beanstalkctl> stats
OK ...
beanstalkctl> exit
```

`exit` 和 `quit` 会退出交互模式。输入 `help` 可以打印命令摘要。同一个交互会话会复用同一
条连接，因此 watched tubes 等连接状态会在命令之间保留。

在真实终端中，交互模式支持行编辑、内存中的命令历史，以及类似 redis-cli 的 Tab 补全。
Tab 会在完整命令和完整选项 token 之间循环，例如 `put`、`peek-ready`、`put --tube`、
`put --ttr`、`reserve --watch` 和 `reserve --timeout`。

## 退出码

- `0`：命令执行完成并打印了 beanstalkd 响应。
- `1`：连接、参数或协议 I/O 错误。

`NOT_FOUND`、`TIMED_OUT`、`DEADLINE_SOON` 等 beanstalkd 业务响应会作为服务端响应
直接打印；当前不会单独改变进程退出码。
