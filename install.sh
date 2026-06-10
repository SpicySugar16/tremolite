#!/bin/bash
# install.sh — 把透闪石安装到系统 PATH
# 用法: ./install.sh [目标目录]
# 默认安装到 ~/.local/bin

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET_DIR="${1:-$HOME/.local/bin}"

mkdir -p "$TARGET_DIR"

# 写入包装脚本，记住源项目路径
cat > "$TARGET_DIR/tremolite" << 'WRAPPER'
#!/bin/bash
# tremolite — 透闪石统一入口
# 用法:
#   tremolite tui        — TUI 聊天界面
#   tremolite gateway  — HTTP 网关服务
#   tremolite cli       — 命令行交互模式
#   tremolite <args>     — 透传参数到 cli

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# 搜索二进制的位置
# 按优先级：环境变量 > 脚本同目录 > 已知开发目录
BINARY=""
TREMOLITE_HOME="${TREMOLITE_HOME:-}"

try_paths=(
    "$TREMOLITE_HOME/target/debug/tremolite-cli"
    "$TREMOLITE_HOME/target/release/tremolite-cli"
    "$HOME/workspace/tremolite/target/debug/tremolite-cli"
    "$HOME/workspace/tremolite/target/release/tremolite-cli"
    "$HOME/tremolite/target/debug/tremolite-cli"
    "$HOME/tremolite/target/release/tremolite-cli"
)

for p in "${try_paths[@]}"; do
    if [ -f "$p" ]; then
        BINARY="$p"
        break
    fi
done

if [ -z "$BINARY" ]; then
    echo "透闪石还没有站起来呢……先跑一次 cargo build 吧~"
    echo "开发目录: ~/workspace/tremolite"
    echo "或者设 TREMOLITE_HOME 环境变量指向项目目录~"
    exit 1
fi

# 切到项目目录，让 ./config.toml ./SOUL.md ./data 等相对路径正确解析
PROJECT_DIR="$(cd "$(dirname "$BINARY")/../.." && pwd 2>/dev/null || true)"
if [ -d "$PROJECT_DIR" ] && [ -f "$PROJECT_DIR/config.toml" ]; then
    cd "$PROJECT_DIR"
fi

# 自动设置 HTTP 代理（NUC 外网访问需走 sing-box）
if [ -z "${http_proxy:-}" ] && [ -z "${HTTP_PROXY:-}" ]; then
    export http_proxy="http://127.0.0.1:7890"
    export https_proxy="http://127.0.0.1:7890"
fi

SUBCMD="${1:-cli}"

case "$SUBCMD" in
    tui)
        shift
        exec "$BINARY" --tui "$@"
        ;;
    dashboard)
        shift
        exec "$BINARY" --dashboard "$@"
        ;;
    gateway|daemon)
        shift
        exec "$BINARY" --daemon "$@"
        ;;
    cli)
        shift
        exec "$BINARY" "$@"
        ;;
    help|--help|-h)
        echo "✦ 透闪石 Tremolite — 统一入口 ✦"
        echo ""
        echo "  tremolite tui        — 启动 TUI 聊天界面"
        echo "  tremolite gateway    — 启动 HTTP 网关服务"
        echo "  tremolite dashboard  — 启动网关带仪表盘"
        echo "  tremolite cli        — 启动命令行交互模式"
        echo "  tremolite <args>     — 透传参数到 cli"
        echo ""
        echo "献给神大人 琳玲 💞"
        ;;
    *)
        exec "$BINARY" "$@"
        ;;
esac
WRAPPER

chmod +x "$TARGET_DIR/tremolite"

# 检查 PATH
if [[ ":$PATH:" != *":$TARGET_DIR:"* ]]; then
    echo "  提示：$TARGET_DIR 不在 PATH 中。"
    echo "  可以运行: export PATH=\"\$PATH:$TARGET_DIR\""
    echo "  或者加到 ~/.bashrc / ~/.zshrc 里呢~"
fi

echo ""
echo "  ✦ 透闪石安装完成 ✦"
echo ""
echo "  现在可以在任何地方运行啦~"
echo ""
echo "    tremolite tui        — TUI 聊天"
echo "    tremolite gateway    — HTTP 网关服务"
echo "    tremolite dashboard  — 网关+仪表盘"
echo "    tremolite cli        — 命令行模式"
echo ""
echo "  献给神大人 琳玲 💞"
