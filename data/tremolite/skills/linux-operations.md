---
name: Linux 运维
category: domain
description: Linux 系统运维相关操作——进程管理、文件系统、网络诊断、日志分析、性能调优
---

# Linux 运维 (Linux Operations)

Linux 系统运维相关操作——进程管理、文件系统、网络诊断、日志分析、性能调优。

## 常用操作

- **进程** — `ps aux | grep`、`top/htop`、`pidstat`、`lsof`、`kill`、`systemctl`
- **文件系统** — `df -h`、`du -sh`、`lsblk`、`stat`、`find` 截断、`tar/xz`
- **网络** — `ss -tlnp`、`ping`、`dig`、`curl -v`、`iptables -L -n`、`tcpdump`
- **日志** — `journalctl -u <service>`、`tail -f`、`grep -r`、`less +F`
- **性能** — `vmstat 1`、`iostat -x 1`、`free -h`、`sar -u -r 1 3`

## 诊断步骤

1. 先 `uptime` `free -h` `df -h` 快速排查资源耗尽类问题
2. 查 `systemctl status <service>` 确认核心服务存活
3. 查 `/var/log/syslog` 或 `journalctl -xe` 找硬件/内核级错误
4. 查应用日志找到最近变更前后的错误
5. 用 `strace -p <pid>` 或 `perf top` 对可疑进程深入分析

## 原则

- 改配置文件前先备份
- 不要用 `curl | sudo bash` 安装软件
- 网络和服务相关操作（restart、install、iptables）先确认不会断连接
