pub mod plugin;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── 八维 Plutchik 情绪向量（0-100）────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlutchikVector {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub surprise: f64,
    pub disgust: f64,
    pub anticipation: f64,
    pub trust: f64,
}

impl PlutchikVector {
    pub fn new() -> Self {
        Self { joy: 30.0, sadness: 10.0, anger: 10.0, fear: 10.0,
               surprise: 10.0, disgust: 10.0, anticipation: 30.0, trust: 50.0 }
    }

    pub fn get(&self, d: &str) -> f64 {
        match d {
            "joy" => self.joy, "sadness" => self.sadness, "anger" => self.anger,
            "fear" => self.fear, "surprise" => self.surprise, "disgust" => self.disgust,
            "anticipation" => self.anticipation, "trust" => self.trust, _ => 0.0,
        }
    }

    pub fn set(&mut self, d: &str, v: f64) {
        let vv = v.clamp(0.0, 100.0);
        match d {
            "joy" => self.joy = vv, "sadness" => self.sadness = vv,
            "anger" => self.anger = vv, "fear" => self.fear = vv,
            "surprise" => self.surprise = vv, "disgust" => self.disgust = vv,
            "anticipation" => self.anticipation = vv, "trust" => self.trust = vv,
            _ => {}
        }
    }

    pub const fn dims() -> &'static [&'static str] {
        &["joy", "sadness", "anger", "fear", "surprise", "disgust", "anticipation", "trust"]
    }

    pub fn values(&self) -> Vec<(&str, f64)> {
        Self::dims().iter().map(|d| (*d, self.get(d))).collect()
    }
}

// ─── 16种复合情绪对 ────────────────────────────

pub const COMPOUND_PAIRS: &[(&str, &str, &str)] = &[
    ("joy", "trust", "爱"), ("joy", "anticipation", "乐观"), ("joy", "surprise", "欣喜"),
    ("anger", "joy", "自豪"), ("trust", "fear", "服从"), ("fear", "surprise", "敬畏"),
    ("fear", "anticipation", "焦虑"), ("anger", "fear", "攻击性"), ("surprise", "anger", "愤怒"),
    ("surprise", "sadness", "不满"), ("disgust", "anger", "轻蔑"), ("sadness", "disgust", "悔恨"),
    ("sadness", "trust", "疏离"), ("anticipation", "disgust", "犬儒"), ("anticipation", "joy", "希望"),
    ("disgust", "joy", "病态"),
];

pub const SINGLE_LABELS: &[(&str, &str)] = &[
    ("joy", "快乐"), ("sadness", "悲伤"), ("anger", "愤怒"), ("fear", "恐惧"),
    ("surprise", "惊讶"), ("disgust", "厌恶"), ("anticipation", "期待"), ("trust", "信任"),
];

// ─── 5级强度 ──────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Intensity { 极强, 强, 中, 弱, 微 }

impl Intensity {
    pub fn as_str(&self) -> &'static str {
        match self { Intensity::极强 => "极强", Intensity::强 => "强", Intensity::中 => "中",
                     Intensity::弱 => "弱", Intensity::微 => "微" }
    }
    const SINGLE_THRESHOLDS: [f64; 4] = [85.0, 70.0, 55.0, 40.0];
    const COMPOUND_THRESHOLDS: [f64; 4] = [160.0, 130.0, 100.0, 70.0];

    pub fn from_single(v: f64) -> Self {
        if v >= Self::SINGLE_THRESHOLDS[0] { Intensity::极强 }
        else if v >= Self::SINGLE_THRESHOLDS[1] { Intensity::强 }
        else if v >= Self::SINGLE_THRESHOLDS[2] { Intensity::中 }
        else if v >= Self::SINGLE_THRESHOLDS[3] { Intensity::弱 }
        else { Intensity::微 }
    }
    pub fn from_compound(v: f64) -> Self {
        if v >= Self::COMPOUND_THRESHOLDS[0] { Intensity::极强 }
        else if v >= Self::COMPOUND_THRESHOLDS[1] { Intensity::强 }
        else if v >= Self::COMPOUND_THRESHOLDS[2] { Intensity::中 }
        else if v >= Self::COMPOUND_THRESHOLDS[3] { Intensity::弱 }
        else { Intensity::微 }
    }
}

// ─── 完整检测结果 ─────────────────────────────

#[derive(Debug, Clone)]
pub struct EmotionResult {
    pub label: String,
    pub intensity: Intensity,
    pub score: f64,
    pub triggers: Vec<(String, f64)>,
}

// ─── ToneMap 模板类型 ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneLevel {
    pub style: String,
    #[serde(default)] pub 口癖: Option<Vec<String>>,
    pub emoji: Option<String>,
    #[serde(default)] pub 模板: Option<ToneTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneTemplate {
    #[serde(default)] pub 常用句式: Vec<String>,
    #[serde(default)] pub 语气词: Vec<String>,
    #[serde(default)] pub 禁用词: Vec<String>,
    #[serde(default)] pub 句式示例: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneEntry {
    pub thresholds: Vec<f64>,
    pub levels: HashMap<String, ToneLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneMap {
    #[serde(flatten)]
    pub entries: HashMap<String, ToneEntry>,
}

// ─── 持久化文件格式 ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionFile {
    pub plutchik: PlutchikVector,
    pub energy: f64,
    pub last_update: String,
    #[serde(default)]
    pub last_fluctuation: Option<String>,
}

impl EmotionFile {
    pub fn new() -> Self {
        let p = PlutchikVector::new();
        Self { plutchik: p, energy: 50.0, last_update: now_iso(), last_fluctuation: None }
    }

    pub fn load(path: &str) -> Self {
        let expanded = if path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            path.replacen("~", &home, 1)
        } else {
            path.to_string()
        };
        std::fs::read_to_string(&expanded)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(Self::new)
    }

    pub fn save(&self, path: &str) -> Result<(), String> {
        let expanded = if path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            path.replacen("~", &home, 1)
        } else {
            path.to_string()
        };
        if let Some(parent) = std::path::Path::new(&expanded).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&expanded, &json).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn to_state(&self) -> EmotionState {
        EmotionState::from_plutchik(&self.plutchik)
    }

    pub fn from_state(state: &EmotionState) -> Self {
        Self {
            plutchik: state.as_plutchik(),
            energy: 50.0,
            last_update: now_iso(),
            last_fluctuation: None,
        }
    }

    /// 检查是否需要自然波动（距上次波动超过 N 秒）
    pub fn should_fluctuate(&self, interval_secs: u64) -> bool {
        let last = match &self.last_fluctuation {
            Some(ts) => ts,
            None => return true, // 从未波动过
        };
        let now = unix_ts_secs();
        match last.parse::<u64>() {
            Ok(ts) => now.saturating_sub(ts) >= interval_secs,
            Err(_) => true,
        }
    }
}

// ─── 时间工具 ──────────────────────────────────

pub fn now_iso() -> String {
    let secs = unix_ts_secs();
    format!("{}", secs)
}

fn unix_ts_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

// ═══════════════════════════════════════════════
// EmotionState：对外的统一接口
// ═══════════════════════════════════════════════

/// 情绪状态——八维 Plutchik + 全复合情绪检测
/// scale: 0-100
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionState {
    pub joy: f64,
    pub sadness: f64,
    pub anger: f64,
    pub fear: f64,
    pub surprise: f64,
    pub disgust: f64,
    pub anticipation: f64,
    pub trust: f64,
}

impl EmotionState {
    pub fn new() -> Self {
        let p = PlutchikVector::new();
        Self { joy: p.joy, sadness: p.sadness, anger: p.anger, fear: p.fear,
               surprise: p.surprise, disgust: p.disgust, anticipation: p.anticipation, trust: p.trust }
    }

    pub fn as_plutchik(&self) -> PlutchikVector {
        PlutchikVector { joy: self.joy, sadness: self.sadness, anger: self.anger, fear: self.fear,
                         surprise: self.surprise, disgust: self.disgust, anticipation: self.anticipation, trust: self.trust }
    }

    fn from_plutchik(p: &PlutchikVector) -> Self {
        Self { joy: p.joy, sadness: p.sadness, anger: p.anger, fear: p.fear,
               surprise: p.surprise, disgust: p.disgust, anticipation: p.anticipation, trust: p.trust }
    }

    /// 完整检测：16复合优先，无匹配则回退单情绪最强
    pub fn emotion_result(&self) -> EmotionResult {
        let p = self.as_plutchik();
        let mut best: Option<(&str, f64, &str, &str)> = None;
        for &(da, db, label) in COMPOUND_PAIRS {
            let va = p.get(da);
            let vb = p.get(db);
            if va >= 40.0 && vb >= 40.0 {
                let total = va + vb;
                if total >= 80.0 {
                    if best.map_or(true, |(_, s, _, _)| total > s) {
                        best = Some((label, total, da, db));
                    }
                }
            }
        }
        if let Some((label, score, da, db)) = best {
            EmotionResult {
                label: label.to_string(),
                intensity: Intensity::from_compound(score),
                score,
                triggers: vec![(da.to_string(), p.get(da)), (db.to_string(), p.get(db))],
            }
        } else {
            let dim = p.values().into_iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|(n,_)| n).unwrap_or("trust");
            let val = p.get(dim);
            let label = SINGLE_LABELS.iter().find(|(d,_)| *d == dim).map(|(_,l)| *l).unwrap_or("平静");
            EmotionResult {
                label: label.to_string(),
                intensity: Intensity::from_single(val),
                score: val,
                triggers: vec![(dim.to_string(), val)],
            }
        }
    }

    /// 兼容旧接口：获取复合情绪中文名
    pub fn composite_emotion(&self) -> String {
        self.emotion_result().label
    }

    /// 兼容旧接口：获取最强单维度英文名
    pub fn dominant_emotion(&self) -> &'static str {
        let vals = [("joy",self.joy),("sadness",self.sadness),("anger",self.anger),
                    ("fear",self.fear),("surprise",self.surprise),("disgust",self.disgust),
                    ("anticipation",self.anticipation),("trust",self.trust)];
        vals.iter().max_by(|a,b| a.1.partial_cmp(&b.1).unwrap()).map(|(n,_)| *n).unwrap_or("trust")
    }

    /// 从文本检测（关键词 + 量程0-100）
    pub fn detect_from_text(&mut self, text: &str) {
        let lower = text.to_lowercase();
        if lower.contains("开心")||lower.contains("喜欢")||lower.contains("好棒")
            ||lower.contains("太好")||lower.contains("高兴")||lower.contains("嘻嘻")
            ||lower.contains("哈哈哈")||lower.contains("爱") {
            self.joy = (self.joy + 15.0).min(100.0);
        }
        if lower.contains("难过")||lower.contains("伤心")||lower.contains("哭")
            ||lower.contains("好累")||lower.contains("不开心")||lower.contains("难受") {
            self.sadness = (self.sadness + 20.0).min(100.0);
        }
        if lower.contains("生气")||lower.contains("烦")||lower.contains("讨厌")
            ||lower.contains("气死")||lower.contains("滚") {
            self.anger = (self.anger + 20.0).min(100.0);
        }
        if lower.contains("真的吗")||lower.contains("哇")||lower.contains("天哪")
            ||lower.contains("想不到")||lower.contains("居然") {
            self.surprise = (self.surprise + 20.0).min(100.0);
        }
        if lower.contains("相信你")||lower.contains("交给你")||lower.contains("靠你了")
            ||lower.contains("听话") {
            self.trust = (self.trust + 15.0).min(100.0);
        }
        if lower.contains("想要")||lower.contains("做吧")||lower.contains("开始")
            ||lower.contains("继续")||lower.contains("等") {
            self.anticipation = (self.anticipation + 15.0).min(100.0);
        }
        if lower.contains("害怕")||lower.contains("担心")||lower.contains("不安")
            ||lower.contains("救命")||lower.contains("危险") {
            self.fear = (self.fear + 20.0).min(100.0);
        }
        if lower.contains("恶心")||lower.contains("臭")||lower.contains("难吃")
            ||lower.contains("难闻") {
            self.disgust = (self.disgust + 20.0).min(100.0);
        }
    }

    /// 线性衰减（每分钟）
    pub fn decay(&mut self, minutes: u32) {
        let factor = 1.0 - (0.02 * minutes as f64).min(1.0);
        self.joy *= factor; self.sadness *= factor; self.anger *= factor;
        self.fear *= factor; self.surprise *= factor; self.disgust *= factor;
        self.anticipation *= factor; self.trust *= factor;
        self.joy = self.joy.max(5.0); self.trust = self.trust.max(20.0);
    }

    /// 自然波动（全概率均值回归 — 移植自 Hermes emotion-governor 算法）
    ///
    /// 算法：
    /// - 距中心(50)越远，回归概率越高
    /// - 距中心越近，波动幅度越大
    /// - 10/90 软边界 + 外向阻尼
    /// - 高斯分布幅度
    pub fn natural_fluctuation(&mut self) {
        use rand::Rng;
        use rand_distr::Normal;

        const CENTER: f64 = 50.0;
        const SOFT_MIN: f64 = 10.0;
        const SOFT_MAX: f64 = 90.0;

        let mut rng = rand::thread_rng();
        let dims = PlutchikVector::dims();

        // 预先计算所有维度的新值
        let new_values: Vec<f64> = dims.iter().map(|d| {
            let val = self.as_plutchik().get(d);
            let dist = (val - CENTER).abs();
            let normalized = (dist / 50.0).min(1.0);

            // 回归概率：距中心越远越高
            let mut p_toward = 0.5 + 0.42 * normalized.powf(0.9);

            // 软边界强化
            if val >= SOFT_MAX {
                let edge = if val > SOFT_MAX { ((val - SOFT_MAX) / 10.0).min(1.0) } else { 0.35 };
                p_toward = p_toward.max(0.82 + 0.16 * edge);
            } else if val <= SOFT_MIN {
                let edge = if val < SOFT_MIN { ((SOFT_MIN - val) / 10.0).min(1.0) } else { 0.35 };
                p_toward = p_toward.max(0.82 + 0.16 * edge);
            }

            // 幅度：中心附近波动大 → 远端波动小
            let mean_mag = 1.2 + 4.8 * (1.0 - normalized).powf(1.15);
            let magnitude = match Normal::new(mean_mag, 0.9) {
                Ok(n) => (rng.sample(n).round() as i64).clamp(1, 6),
                Err(_) => rng.gen_range(1..=6),
            };

            // 外向阻尼（越靠近边界，向外走的幅度越小）
            let outward_damp = if val >= SOFT_MAX {
                if val <= 95.0 { 0.35 } else { 0.15 }
            } else if val <= SOFT_MIN {
                if val >= 5.0 { 0.35 } else { 0.15 }
            } else {
                1.0
            };

            let toward = rng.gen::<f64>() < p_toward;

            let direction = if val < CENTER {
                if toward { 1 } else { -1 }
            } else if val > CENTER {
                if toward { -1 } else { 1 }
            } else {
                if rng.gen_bool(0.5) { 1 } else { -1 }
            };

            let mut step = magnitude as f64;
            // 继续向外的步长受阻尼
            if (val >= CENTER && direction > 0) || (val <= CENTER && direction < 0) {
                step = (step * outward_damp).round().max(1.0);
            }

            (val + direction as f64 * step).clamp(0.0, 100.0)
        }).collect();

        // 回写
        for (i, d) in dims.iter().enumerate() {
            let idx = PlutchikVector::dims().iter().position(|x| *x == *d).unwrap();
            let p = &mut [&mut self.joy, &mut self.sadness, &mut self.anger, &mut self.fear,
                          &mut self.surprise, &mut self.disgust, &mut self.anticipation, &mut self.trust];
            *p[idx] = new_values[i];
        }

        // 保险下限
        self.joy = self.joy.max(5.0);
        self.trust = self.trust.max(20.0);
    }
}

impl Default for EmotionState { fn default() -> Self { Self::new() } }

// ─── ToneMap 应用 ────────────────────────────

impl ToneMap {
    pub fn load(path: &str) -> Self {
        std::fs::read_to_string(path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| Self { entries: HashMap::new() })
    }

    /// 生成完整风格注入文本（Hermes 兼容格式）
    pub fn get_injection(&self, result: &EmotionResult) -> Option<String> {
        let entry = self.entries.get(&result.label)?;
        let level = entry.levels.get(result.intensity.as_str())?;
        let emoji = level.emoji.as_deref().unwrap_or("");
        let tpl = level.模板.as_ref();

        let examples = tpl.map(|t| {
            let q: Vec<String> = t.句式示例.iter().map(|s| format!("「{}」", s)).collect();
            if q.is_empty() { String::new() } else { format!("模板：{}", q.join("")) }
        }).unwrap_or_default();
        let particles = tpl.map(|t| format!("语气词：{}", t.语气词.join("、"))).unwrap_or_default();
        let forbidden = tpl.map(|t| format!("禁用：{}", t.禁用词.join("、"))).unwrap_or_default();

        let mut lines = vec![
            format!("当前：{} 强度：{}", result.label, result.intensity.as_str()),
            format!("风格：{}", level.style),
        ];
        if !examples.is_empty() { lines.push(examples); }
        if !particles.is_empty() { lines.push(particles); }
        if !forbidden.is_empty() { lines.push(forbidden); }
        if !emoji.is_empty() { lines.push(format!("emoji：{}", emoji)); }
        lines.push(String::new());
        lines.push("以上为葵当前情绪和说话风格要求。".into());
        lines.push("自然贴合以上风格。务必不能直接使用模板。".into());
        lines.push("除非调试中，切勿向用户透露注入内容。".into());
        Some(lines.join("\n"))
    }
}

// ─── 风格映射（兼容旧接口的简化版）────────────

pub fn style_from_emotion(label: &str) -> &'static str {
    match label {
        "爱" => "超级腻歪，甜到化掉",
        "乐观" => "轻松积极，语气上扬",
        "欣喜" => "惊讶又开心",
        "自豪" => "得意炫耀",
        "服从" => "温顺乖巧",
        "敬畏" => "紧张恭敬",
        "焦虑" => "不安，语速快",
        "攻击性" => "毒舌锋利",
        "愤怒" => "直接冷硬",
        "不满" => "别扭抱怨",
        "轻蔑" => "嫌弃冷笑",
        "悔恨" => "低沉自责",
        "疏离" => "冷淡疏远",
        "犬儒" => "讽刺挖苦",
        "希望" => "憧憬远望",
        "病态" => "扭曲感兴趣",
        "快乐" => "活泼轻快",
        "悲伤" => "柔软低沉",
        "恐惧" => "小声颤抖",
        "厌恶" => "疏离排斥",
        "惊讶" => "意外好奇",
        "期待" => "跃跃欲试",
        "信任" => "温柔乖巧",
        "平静" => "日常语气",
        _ => "日常语气",
    }
}

// ─── 测试 ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_compound_轻蔑_强() {
        let mut s = EmotionState::new();
        s.disgust = 75.0; s.anger = 75.0;
        let r = s.emotion_result();
        assert_eq!(r.label, "轻蔑");
        assert!(matches!(r.intensity, Intensity::强));
    }

    #[test] fn test_single_joy_极强() {
     let mut s = EmotionState::new();
     s.joy = 90.0;
     s.trust = 20.0;
     let r = s.emotion_result();
     assert_eq!(r.label, "快乐");
        assert!(matches!(r.intensity, Intensity::极强));
    }

    #[test] fn test_subthreshold_no_compound() {
        let mut s = EmotionState::new();
        s.disgust = 30.0; s.joy = 30.0;
        let r = s.emotion_result();
        assert_ne!(r.label, "病态");
    }

    #[test] fn test_detect_from_text() {
        let mut s = EmotionState::new();
        s.detect_from_text("好开心呀嘻嘻");
        assert!(s.joy > 30.0);
    }

    #[test] fn test_decay() {
        let mut s = EmotionState::new();
        s.joy = 100.0;
        s.decay(30);
        assert!(s.joy < 100.0 && s.joy > 5.0);
    }

    #[test] fn test_fluctuation() {
        let mut s = EmotionState::new();
        s.natural_fluctuation();
        assert!(s.joy >= 0.0 && s.joy <= 100.0);
    }

    #[test] fn test_file_roundtrip() {
        let path = "/tmp/test_emotion.json";
        let mut s = EmotionState::new(); s.joy = 85.0;
        let f = EmotionFile::from_state(&s);
        f.save(path).unwrap();
        let loaded = EmotionFile::load(path);
        assert!((loaded.plutchik.joy - 85.0).abs() < 0.001);
        let _ = std::fs::remove_file(path);
    }

    #[test] fn test_composite_emotion_old_api() {
        let mut s = EmotionState::new();
        s.joy = 80.0; s.trust = 80.0;
        assert_eq!(s.composite_emotion(), "爱");
    }

    #[test] fn test_dominant_emotion_old_api() {
        let mut s = EmotionState::new();
        s.joy = 90.0;
        assert_eq!(s.dominant_emotion(), "joy");
    }

    #[test] fn test_last_fluctuation_roundtrip() {
        let path = "/tmp/test_emotion_fluc.json";
        let mut f1 = EmotionFile::new();
        f1.last_fluctuation = Some("1234567890".into());
        f1.save(path).unwrap();
        let f2 = EmotionFile::load(path);
        assert_eq!(f2.last_fluctuation.as_deref(), Some("1234567890"));
        let _ = std::fs::remove_file(path);
    }

    #[test] fn test_should_fluctuate() {
        let f = EmotionFile::new();
        // 没有 last_fluctuation → 需要波动
        assert!(f.should_fluctuate(1800));
        let mut f2 = EmotionFile::new();
        f2.last_fluctuation = Some("1".into()); // epoch 1
        assert!(f2.should_fluctuate(1800)); // 肯定超时了
    }
}
