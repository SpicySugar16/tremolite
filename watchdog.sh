#!/bin/bash
# Tremolite watchdog — 让透闪石跌倒了自己爬起来呢
# 用法: ./watchdog.sh [--daemon] [--port PORT] [--config CONFIG_PATH]

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$SCRIPT_DIR/target/debug/tremolite-cli"
LOG_DIR="$SCRIPT_DIR/logs"

# 默认参数
DAEMON=""
PORT=8080
CONFIG=""
MAX_RESTARTS=10
RESTART_DELAY=2

# 解析参数
while [[ $# -gt 0 ]]; do
    case "$1" in
        --daemon) DAEMON="--daemon" ;;
        --port) PORT="$2"; shift ;;
        --config) CONFIG="$2"; shift ;;
        --max-restarts) MAX_RESTARTS="$2"; shift ;;
        *) echo "未知参数: $1"; exit 1 ;;
    esac
    shift
done

mkdir -p "$LOG_DIR"
RESTART_COUNT=0

echo "  ═══════════════════════════════════════════"
echo "    透闪石 Watchdog 已启动"
echo "    二进制: $BINARY"
echo "    端口:   $PORT"
echo "    模式:   ${DAEMON:-交互式}"
echo "    最大重启次数: $MAX_RESTARTS"
echo "  ═══════════════════════════════════════════"
echo ""

while true; do
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] 启动透闪石 (第 $RESTART_COUNT 次)..."
    
    CMD="$BINARY $DAEMON --port $PORT"
    if [[ -n "$CONFIG" ]]; then
        CMD="$CMD $CONFIG"
    fi
    
    # 运行二进制，捕获退出码
    $CMD
    EXIT_CODE=$?
    
    if [[ $EXIT_CODE -eq 0 ]]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] 透闪石正常退出。Watchdog 也退出。"
        exit 0
    fi
    
    RESTART_COUNT=$((RESTART_COUNT + 1))
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] 透闪石异常退出 (code: $EXIT_CODE)，重启次数: $RESTART_COUNT"
    
    if [[ $RESTART_COUNT -ge $MAX_RESTARTS ]]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] 超过最大重启次数 ($MAX_RESTARTS)，Watchdog 退出。"
        exit 1
    fi
    
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] 等待 ${RESTART_DELAY} 秒后重启..."
    sleep "$RESTART_DELAY"
done
