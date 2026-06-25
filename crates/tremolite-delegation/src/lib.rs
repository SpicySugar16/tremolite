use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// ─── 委派模式 ─────────────────────────────────

/// 委派模式——告诉引擎怎么干活
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DelegateMode {
    /// 自己 spawn 子 tremolite 进程（完整 agent 推理）
    Tremolite,
    /// 调用外部 ACP 工具（OpenCode 等）
    AcpTool {
        /// CLI 名称，如 opencode、copilot
        command: String,
        /// CLI 参数
        args: Vec<String>,
    },
    /// 直接运行 shell 命令
    Shell {
        command: String,
    },
}

// ─── 任务上下文 ────────────────────────────────

/// 交给子进程的任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// 任务目标——做什么
    pub goal: String,
    /// 背景信息——知道什么
    pub context: String,
    /// 工作目录
    pub workdir: String,
    /// 传给子 agent 的 SOUL 或 system prompt 片段
    pub soul_fragment: String,
    /// 期待的产出
    pub expected_output: String,
    /// 最大执行秒数
    pub timeout_secs: u64,
}

impl TaskContext {
    pub fn new(goal: &str, context: &str) -> Self {
        Self {
            goal: goal.to_string(),
            context: context.to_string(),
            workdir: String::new(),
            soul_fragment: String::new(),
            expected_output: String::new(),
            timeout_secs: 300,
        }
    }

    pub fn with_workdir(mut self, dir: &str) -> Self {
        self.workdir = dir.to_string();
        self
    }

    pub fn with_soul(mut self, soul: &str) -> Self {
        self.soul_fragment = soul.to_string();
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_expected(mut self, expected: &str) -> Self {
        self.expected_output = expected.to_string();
        self
    }
}

// ─── 任务状态 ────────────────────────────────

#[derive(Debug)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed(String),
    Failed(String),
    Cancelled,
}

#[derive(Debug)]
pub struct TaskHandle {
    pub mode: DelegateMode,
    pub context: TaskContext,
    pub status: TaskStatus,
    pub started_at: Option<Instant>,
    pub elapsed: Option<Duration>,
    child: Option<Child>,
}

impl TaskHandle {
    pub fn is_done(&self) -> bool {
        matches!(self.status, TaskStatus::Completed(_) | TaskStatus::Failed(_) | TaskStatus::Cancelled)
    }

    pub fn result(&self) -> Option<&str> {
        match &self.status {
            TaskStatus::Completed(r) => Some(r.as_str()),
            _ => None,
        }
    }

    /// 等待任务完成（阻塞）
    pub fn wait(&mut self, timeout: Duration) -> Result<String, String> {
        if self.is_done() {
            return match &self.status {
                TaskStatus::Completed(r) => Ok(r.clone()),
                TaskStatus::Failed(e) => Err(format!("task failed: {e}")),
                _ => Err("task cancelled".into()),
            };
        }
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                self.cancel();
                return Err("delegation: task timed out".into());
            }
            self.poll()?;
            if self.is_done() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        match &self.status {
            TaskStatus::Completed(r) => Ok(r.clone()),
            TaskStatus::Failed(e) => Err(format!("task failed: {e}")),
            _ => Err("task cancelled".into()),
        }
    }

    /// 检查一次子进程状态
    fn poll(&mut self) -> Result<(), String> {
        let child = match self.child.as_mut() {
            Some(c) => c,
            None => return Err("no child process".into()),
        };

        match child.try_wait() {
            Ok(Some(status)) => {
                // 进程已退出——读取剩余输出
                self.elapsed = self.started_at.map(|s| s.elapsed());
                self.child = None;

                if status.success() {
                    // Tremolite 模式的输出已经在 recv 中收集
                    // 这里只检查 exit code
                } else {
                    let _ = self.status = TaskStatus::Failed(
                        format!("child exited with code {}", status.code().unwrap_or(-1))
                    );
                }
                Ok(())
            }
            Ok(None) => {
                // 仍在运行
                Ok(())
            }
            Err(e) => {
                self.status = TaskStatus::Failed(format!("waitpid error: {e}"));
                Err(format!("waitpid error: {e}"))
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.status = TaskStatus::Cancelled;
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if self.child.is_some() && !self.is_done() {
            self.cancel();
        }
    }
}

// ─── 委派引擎 ─────────────────────────────────

/// 委派引擎——创建和管理子任务
pub struct DelegationEngine;

impl DelegationEngine {
    /// 创建一个委派任务（不启动）
    pub fn prepare(mode: DelegateMode, context: TaskContext) -> TaskHandle {
        TaskHandle {
            mode,
            context,
            status: TaskStatus::Pending,
            started_at: None,
            elapsed: None,
            child: None,
        }
    }

    /// 启动任务（阻塞版——等待完成）
    pub fn spawn_and_wait(
        mode: DelegateMode,
        context: TaskContext,
        timeout: Duration,
    ) -> Result<String, String> {
        let mut handle = Self::spawn(mode, context)?;
        handle.wait(timeout)
    }

    /// 启动任务（异步版——返回 handle）
    pub fn spawn(mode: DelegateMode, context: TaskContext) -> Result<TaskHandle, String> {
        let mut handle = Self::prepare(mode, context);
        Self::do_spawn(&mut handle)?;
        Ok(handle)
    }

    fn do_spawn(handle: &mut TaskHandle) -> Result<(), String> {
        handle.started_at = Some(Instant::now());
        handle.status = TaskStatus::Running;

        // 克隆 mode，避免 borrow 冲突
        let mode = match &handle.mode {
            DelegateMode::Tremolite => DelegateMode::Tremolite,
            DelegateMode::AcpTool { command, args } => {
                DelegateMode::AcpTool {
                    command: command.clone(),
                    args: args.clone(),
                }
            }
            DelegateMode::Shell { command } => {
                DelegateMode::Shell {
                    command: command.clone(),
                }
            }
        };

        match mode {
            DelegateMode::Tremolite => {
                Self::spawn_tremolite(handle)?;
            }
            DelegateMode::AcpTool { command, args } => {
                Self::spawn_acp(handle, &command, &args)?;
            }
            DelegateMode::Shell { command } => {
                Self::spawn_shell(handle, &command)?;
            }
        }
        Ok(())
    }

    /// Tremolite 子进程模式：
    /// 启动 `tremolite --delegate`，通过 stdin/stdout JSON 行协议通信
    fn spawn_tremolite(handle: &mut TaskHandle) -> Result<(), String> {
        let ctx = &handle.context;
        let mut child = Command::new("tremolite")
            .arg("--delegate")
            .arg("--session")
            .arg(format!(
                "delegate-{}",
                ctx.goal.chars().take(20).collect::<String>()
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn tremolite: {e}"))?;

        // 发送任务上下文
        if let Some(stdin) = child.stdin.as_mut() {
            let json = serde_json::to_string(ctx).map_err(|e| e.to_string())?;
            writeln!(stdin, "{json}").map_err(|e| format!("write stdin: {e}"))?;
            stdin.flush().ok();
        }
        // 关闭 stdin pipe——告诉子进程没有更多输入了
        // 否则子进程的 read_line 会一直阻塞等待下一个换行符
        child.stdin.take();

        // 读取结果（阻塞一次读取）
        let stdout = child.stdout.take()
            .ok_or_else(|| "stdout not available".to_string())?;
        let mut reader = BufReader::new(stdout);
        let mut result_line = String::new();
        reader.read_line(&mut result_line)
            .map_err(|e| format!("read result: {e}"))?;

        let exit_status = child.wait().map_err(|e| format!("wait: {e}"))?;

        if exit_status.success() {
            handle.status = TaskStatus::Completed(result_line.trim().to_string());
        } else {
            handle.status = TaskStatus::Failed(result_line.trim().to_string());
        }
        handle.elapsed = handle.started_at.map(|s| s.elapsed());
        handle.child = None;

        Ok(())
    }

    /// OpenCode / ACP 模式：
    /// 用 stdin/stdout JSON 行协议与外部工具通信
    fn spawn_acp(handle: &mut TaskHandle, command: &str, args: &[String]) -> Result<(), String> {
        let ctx = &handle.context;
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()
            .map_err(|e| format!("spawn {command}: {e}"))?;

        // ACP 协议：第一行发送目标，后续可发送任务上下文
        if let Some(stdin) = child.stdin.as_mut() {
            let prompt = format!(
                "Task: {}\n\nContext:\n{}\n\nWorkdir: {}\nExpected: {}",
                ctx.goal, ctx.context, ctx.workdir, ctx.expected_output
            );
            // ACP 接受 JSON 行消息或纯文本
            writeln!(stdin, "{prompt}").map_err(|e| format!("write stdin: {e}"))?;
            stdin.flush().ok();
        }

        // 这里简化处理：非阻塞等一段时间
        // 实际应该用更复杂的协议，但为了先跑起来，用超时读取
        let timeout = Duration::from_secs(ctx.timeout_secs);
        let start = Instant::now();

        // 读取输出直到超时或进程退出
        let mut output = String::new();
        if let Some(stdout) = child.stdout.as_mut() {
            let mut reader = BufReader::new(stdout);
            let mut buffer = String::new();
            loop {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    break;
                }
                // 非阻塞读取
                match reader.read_line(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        output.push_str(&buffer);
                        buffer.clear();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        return Err(format!("read output: {e}"));
                    }
                }
            }
        }

        let exit_status = child.wait().ok();
        handle.elapsed = Some(start.elapsed());

        if output.is_empty() && exit_status.map_or(false, |s| !s.success()) {
            handle.status = TaskStatus::Failed(
                format!("{command} exited with code {}", exit_status.unwrap().code().unwrap_or(-1))
            );
        } else {
            handle.status = TaskStatus::Completed(output.trim().to_string());
        }
        handle.child = None;

        Ok(())
    }

    /// Shell 模式：执行命令并捕获 stdout
    fn spawn_shell(handle: &mut TaskHandle, command: &str) -> Result<(), String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| format!("shell exec: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        handle.elapsed = handle.started_at.map(|s| s.elapsed());
        handle.child = None;

        if output.status.success() {
            handle.status = TaskStatus::Completed(stdout);
        } else {
            let err_msg = if stderr.is_empty() { stdout } else { stderr };
            handle.status = TaskStatus::Failed(
                format!("exit={} msg={}", output.status.code().unwrap_or(-1), err_msg.trim())
            );
        }

        Ok(())
    }
}

// ─── 测试 ────────────────────────────────────

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    // ── 单元测试：基础 Shell 委派 ─────────────────

    #[test]
    fn test_shell_echo() {
        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell {
                command: "echo hello".into(),
            },
            TaskContext::new("echo test", ""),
            Duration::from_secs(5),
        );
        assert!(result.is_ok(), "echo failed: {:?}", result);
    }

    #[test]
    fn test_shell_fail() {
        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell {
                command: "exit 1".into(),
            },
            TaskContext::new("fail test", ""),
            Duration::from_secs(5),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_shell_multi_line_output() {
        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell {
                command: "printf 'line1\\nline2\\nline3'".into(),
            },
            TaskContext::new("multi line output", ""),
            Duration::from_secs(5),
        );
        assert!(result.is_ok(), "multi-line failed: {:?}", result);
        let output = result.unwrap();
        assert!(output.contains("line1"), "missing line1: {output}");
        assert!(output.contains("line2"), "missing line2: {output}");
        assert!(output.contains("line3"), "missing line3: {output}");
    }

    // ── 集成测试：opencode ACP 委派 ─────────
    // 需要 opencode CLI + DEEPSEEK_API_KEY 环境变量
    // 运行前执行: source ~/.tremolite/.env
    // 运行方式: cargo test -- --ignored

    /// 模拟 DelegationModule 的 opencode 路径：shell 模式执行 opencode run
    #[test]
    #[ignore]
    fn test_opencode_echo() {
        let goal = "echo 'opencode委派测试成功'".to_string();
        let encoded = goal.replace('"', "\\\"");
        let command = format!(
            "opencode run --dangerously-skip-permissions --format json -m deepseek/deepseek-v4-flash \"{encoded}\""
        );
        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell { command },
            TaskContext::new(&goal, "opencode 集成测试"),
            Duration::from_secs(120),
        );
        assert!(result.is_ok(), "opencode echo failed: {:?}", result);
        let output = result.unwrap();
        assert!(output.contains("成功"), "response missing success keyword: {output}");
    }

    /// 测试 opencode 文件创建
    #[test]
    #[ignore]
    fn test_opencode_file_create() {
        let goal = "echo '透闪石ACP委派文件测试成功' > /tmp/delegate_acp_test.txt".to_string();
        let encoded = goal.replace('"', "\\\"");
        let command = format!(
            "opencode run --dangerously-skip-permissions --format json -m deepseek/deepseek-v4-flash \"{encoded}\""
        );
        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell { command },
            TaskContext::new("opencode 文件创建测试", ""),
            Duration::from_secs(120),
        );
        assert!(result.is_ok(), "opencode file create failed: {:?}", result);
    }

    /// 完整链路：模拟 DelegationModule::execute_tool("delegate_task", {mode:"opencode",...})
    #[test]
    #[ignore]
    fn test_full_delegation_opencode_path() {
        let goal = "请运行bash命令: echo 'delegate_task(opencode)全路径测试通过'".to_string();
        let encoded_goal = goal.replace('"', "\\\"");
        let command = format!(
            "opencode run --dangerously-skip-permissions --format json -m deepseek/deepseek-v4-flash \"{encoded_goal}\""
        );
        let ctx = TaskContext::new(&goal, "完整链路测试: 模拟顶级delegate_task工具调用")
            .with_workdir("/tmp")
            .with_timeout(120);

        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Shell { command },
            ctx,
            Duration::from_secs(120),
        );
        assert!(result.is_ok(), "full path failed: {:?}", result);
        let output = result.unwrap();
        assert!(output.contains("测试通过"), "response missing: {output}");
    }

    /// Tremolite 子进程委派模式：通过 spawn_tremolite IPC 协议通信
    /// 测试 tremolite --delegate CLI handler + stdio JSON 行协议
    /// 需要 `tremolite` 在 PATH 中（含 --delegate handler 的构建）
    #[test]
    #[ignore]
    fn test_tremolite_delegate_ipc() {
        let goal = "echo 'tremolite-delegate-IPC-链路通畅'".to_string();
        let ctx = TaskContext::new(&goal, "IPC 全链路测试: tremolite --delegate 协议")
            .with_workdir("/tmp")
            .with_timeout(30);

        let result = DelegationEngine::spawn_and_wait(
            DelegateMode::Tremolite,
            ctx,
            Duration::from_secs(60),
        );
        assert!(result.is_ok(), "tremolite delegate IPC failed: {:?}", result);
        let output = result.unwrap();
        // handle_delegate 从 stdout 取第一行，应该包含 echo 的输出
        assert!(output.contains("IPC"), "IPC test failure: {output}");
    }
}
