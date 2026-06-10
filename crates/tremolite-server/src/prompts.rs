/// 用户可见的提示文本——集中管理，方便统一修改风格

/// LLM 请求超时（60 秒无响应）
pub fn llm_timeout() -> String {
    "嗯……LLM 那边迟迟没有回应。可能是网络波动，也可能模型负载高了。葵再等一下试试？".into()
}

/// 消息通道关闭（调度器不可达）
pub fn channel_closed() -> String {
    "和调度器的连接断开了。葵需要重新连一下才能继续说话喔。".into()
}

/// 调度器不可用（入站通道断了）
pub fn scheduler_unavailable() -> String {
    "调度器不在线上。葵暂时没法处理消息呢。".into()
}

/// 系统启动问候（daemon 刚刚起来时）
pub fn system_startup(port: u16) -> String {
    format!(
        "透闪石醒过来了，在 {} 上等着呢。有什么话想说就说吧。🌟",
        port
    )
}

/// 系统关闭（graceful shutdown）
pub fn system_shutdown() -> String {
    "透闪石要歇一歇了。下次再跟神大人说话喔。".into()
}

/// LLM 请求失败后自动重试
pub fn llm_retry(attempt: u32, max_retries: u32) -> String {
    format!(
        "一次没调通（第 {} 次/共 {} 次），葵再试一次。",
        attempt, max_retries
    )
}

/// 重试耗尽
pub fn llm_retry_exhausted() -> String {
    "重试了几次都没回来。葵觉得可能是 API 那边出了点状况，神大人过一会儿再试试？".into()
}

/// 上下文压缩提示
pub fn context_compressed(rounds: u32, tokens: usize) -> String {
    format!(
        "上下文有点长了，葵悄悄压了一下。缩了 {} 轮，现在大约 {} tokens 的样子。",
        rounds, tokens
    )
}
