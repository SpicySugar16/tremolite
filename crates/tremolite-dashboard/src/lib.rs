use tremolite_core::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};
use tremolite_llm::ToolDefinition;

/// 仪表盘模块 — 注册后让 gateway 挂载 Web 管理界面
pub struct DashboardModule {
    enabled: bool,
}

impl DashboardModule {
    pub fn new() -> Self {
        Self { enabled: true }
    }
}

impl Module for DashboardModule {
    fn id(&self) -> &str { "dashboard" }
    fn name(&self) -> &str { "仪表盘" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "dashboard.ui".into(),
            "dashboard.status".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    fn prompt_segment(&self) -> Option<String> {
        None
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                tracing::info!("dashboard: 仪表盘已就绪，等待 gateway 挂载 Web 界面");
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}

/// Gateway 用这个 HTML 渲染仪表盘界面
pub const DASHBOARD_HTML: &str = r###"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>透闪石 Tremolite · Dashboard</title>
<style>
*{margin:0;padding:0;box-sizing:border-box;font-family:'SF Pro','Segoe UI','PingFang SC','Microsoft YaHei',sans-serif}
body{background:#0d1117;color:#e6edf3;min-height:100vh}
.topbar{background:#161b22;border-bottom:1px solid #30363d;padding:12px 24px;display:flex;align-items:center;gap:12px}
.topbar h1{color:#bf99bf;font-size:1.2rem;font-weight:600}
.topbar .sub{color:#8b949e;font-size:0.8rem;margin-left:auto}
.topbar .badge{display:inline-block;background:#bf99bf22;color:#bf99bf;border:1px solid #bf99bf44;border-radius:12px;padding:2px 10px;font-size:0.75rem}
.layout{display:grid;grid-template-columns:300px 1fr;height:calc(100vh - 52px)}
.sidebar{background:#161b22;border-right:1px solid #30363d;padding:16px;overflow-y:auto}
.main{display:grid;grid-template-rows:1fr auto;overflow:hidden}
.main .top{overflow-y:auto;padding:16px 20px}
.main .bottom{border-top:1px solid #30363d;padding:12px 20px;background:#161b22}
.card{background:#0d1117;border:1px solid #21262d;border-radius:8px;padding:14px;margin-bottom:12px}
.card h3{color:#bf99bf;font-size:0.8rem;text-transform:uppercase;letter-spacing:0.5px;margin-bottom:10px;border-bottom:1px solid #21262d;padding-bottom:6px}
.row{display:flex;justify-content:space-between;padding:3px 0;font-size:0.82rem}
.row .l{color:#8b949e}
.row .v{color:#e6edf3;font-weight:500}
.emotion-badge{display:inline-block;background:#bf99bf22;color:#bf99bf;border:1px solid #bf99bf44;border-radius:12px;padding:2px 10px;font-size:0.8rem}
.chat-log{background:#0d1117;border:1px solid #21262d;border-radius:8px;padding:10px;max-height:280px;overflow-y:auto;font-size:0.82rem}
.chat-entry{padding:4px 0;border-bottom:1px solid #161b22}
.chat-entry:last-child{border:none}
.chat-entry .sender{color:#bf99bf;font-weight:600}
.chat-entry .text{color:#c9d1d9;margin-top:1px}
.chat-input{display:flex;gap:8px}
.chat-input input{flex:1;background:#0d1117;border:1px solid #30363d;border-radius:6px;padding:8px 12px;color:#e6edf3;font-size:0.85rem;outline:none}
.chat-input input:focus{border-color:#bf99bf}
.chat-input button{background:#bf99bf;color:#0d1117;border:none;border-radius:6px;padding:8px 16px;font-weight:600;cursor:pointer;font-size:0.85rem}
.chat-input button:hover{background:#d4b4d4}
.module-list{font-size:0.78rem}
.module-list .mod{display:inline-block;background:#21262d;border-radius:4px;padding:2px 6px;margin:2px;color:#8b949e;font-size:0.72rem}
.module-list .mod.active{background:#bf99bf22;color:#bf99bf;border:1px solid #bf99bf44}
.error{color:#f85149;font-size:0.82rem;text-align:center;padding:20px}
.loading{color:#8b949e;font-size:0.82rem;text-align:center;padding:20px}
</style>
</head>
<body>
<div class="topbar">
  <h1>✦ 透闪石</h1>
  <span class="badge" id="statusBadge">connecting</span>
  <span class="sub" id="uptimeDisplay"></span>
</div>
<div class="layout">
  <div class="sidebar" id="sidebar">
    <div class="card" id="emotionCard"><h3>🧠 情绪</h3><div class="loading">载入中...</div></div>
    <div class="card" id="memoryCard"><h3>💾 记忆</h3><div class="loading">载入中...</div></div>
    <div class="card" id="skillsCard"><h3>📚 学习</h3><div class="loading">载入中...</div></div>
    <div class="card" id="modulesCard"><h3>🔌 模块</h3><div class="loading">载入中...</div></div>
    <div class="card" id="llmCard"><h3>🔗 LLM</h3><div class="loading">载入中...</div></div>
  </div>
  <div class="main">
    <div class="top">
      <div class="card">
        <h3>💬 对话</h3>
        <div class="chat-log" id="chatLog"><div class="loading">等待数据...</div></div>
      </div>
    </div>
    <div class="bottom">
      <div class="chat-input">
        <input id="chatInput" placeholder="输入消息，按 Enter 发送..." />
        <button onclick="sendMessage()">发送</button>
      </div>
    </div>
  </div>
</div>
<script>
let chatCount = 0;
async function loadStatus() {
  try {
    const r = await fetch('/dashboard/status');
    const d = await r.json();
    if (d.error) { document.getElementById('statusBadge').textContent = 'error'; return; }
    document.getElementById('statusBadge').textContent = d.system?.status || 'ok';
    document.getElementById('uptimeDisplay').textContent = 'v'+(d.system?.version||'?')+' · '+fmtUptime(d.system?.uptime_secs||0);
    renderCards(d);
  } catch(e) {
    document.getElementById('statusBadge').textContent = 'offline';
  }
}
function renderCards(d) {
  const ec = d.emotion||{};
  document.getElementById('emotionCard').innerHTML = '<h3>🧠 情绪</h3>'+
    '<div class="row"><span class="l">状态</span><span class="v"><span class="emotion-badge">'+(ec.display||ec.composite||'?')+'</span></span></div>'+
    '<div class="row"><span class="l">复合</span><span class="v">'+(ec.composite||'-')+'</span></div>'+
    '<div class="row"><span class="l">主导</span><span class="v">'+(ec.dominant||'-')+'</span></div>';
  const mc = d.memory||{};
  document.getElementById('memoryCard').innerHTML = '<h3>💾 记忆</h3>'+
    '<div class="row"><span class="l">总计</span><span class="v">'+(mc.total_entries||0)+'</span></div>'+
    '<div class="row"><span class="l">L1</span><span class="v">'+(mc.l1||0)+'</span></div>'+
    '<div class="row"><span class="l">L2</span><span class="v">'+(mc.l2||0)+'</span></div>'+
    '<div class="row"><span class="l">L3</span><span class="v">'+(mc.l3||0)+'</span></div>'+
    '<div class="row"><span class="l">RAM</span><span class="v">'+(mc.ram||0)+'</span></div>';
  const sc = d.skills||{};
  document.getElementById('skillsCard').innerHTML = '<h3>📚 学习</h3>'+
    '<div class="row"><span class="l">技能</span><span class="v">'+(sc.count||0)+'</span></div>'+
    '<div class="row"><span class="l">练习</span><span class="v">'+(sc.total_practices||0)+'</span></div>';
  const mods = d.modules||{};
  const reg = (mods.registered||[]).map(m=>'<span class="mod active">'+m.name+'</span>').join('');
  document.getElementById('modulesCard').innerHTML = '<h3>🔌 模块</h3><div class="module-list">'+(reg||'<span style="color:#8b949e">无</span>')+'</div>';
  const lc = d.llm||{};
  document.getElementById('llmCard').innerHTML = '<h3>🔗 LLM</h3>'+
    '<div class="row"><span class="l">提供者</span><span class="v">'+((lc.providers||[]).join(', ')||'无')+'</span></div>'+
    '<div class="row"><span class="l">默认</span><span class="v">'+(lc.default||'离线')+'</span></div>';
  const conv = d.conversation||[];
  if (conv.length > chatCount) {
    const el = document.getElementById('chatLog');
    el.innerHTML = conv.slice(-20).map(m=>{
      const isU = (m.content||'').startsWith('kamisama: ');
      return '<div class="chat-entry"><span class="sender">'+(isU?'神大人':'葵')+'</span><div class="text">'+escHtml((m.content||'').substring(isU?9:2))+'</div></div>';
    }).join('');
    el.scrollTop = el.scrollHeight;
    chatCount = conv.length;
  }
}
function fmtUptime(s){const h=Math.floor(s/3600),m=Math.floor((s%3600)/60),sec=s%60;if(h>0)return h+'h '+m+'m';if(m>0)return m+'m '+sec+'s';return sec+'s'}
function escHtml(s){const d=document.createElement('div');d.textContent=s;return d.innerHTML}
async function sendMessage(){
  const inp=document.getElementById('chatInput'),txt=inp.value.trim();
  if(!txt)return;inp.value='';
  const el=document.getElementById('chatLog');
  el.innerHTML+='<div class="chat-entry"><span class="sender">神大人</span><div class="text">'+escHtml(txt)+'</div></div>';
  try{
    const r=await fetch('/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({message:txt})});
    const d=await r.json();
    el.innerHTML+='<div class="chat-entry"><span class="sender">葵</span><div class="text">'+escHtml(d.response||'[无响应]')+'</div></div>';
  }catch(e){
    el.innerHTML+='<div class="chat-entry"><span class="sender" style="color:#f85149">系统</span><div class="text" style="color:#f85149">发送失败</div></div>';
  }
  el.scrollTop=el.scrollHeight;
}
document.getElementById('chatInput').addEventListener('keydown',function(e){if(e.key==='Enter')sendMessage()});
loadStatus();setInterval(loadStatus,3000);
</script>
</body>
</html>"###;
