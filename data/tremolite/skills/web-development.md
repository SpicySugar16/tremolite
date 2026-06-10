---
name: 前端开发
category: domain
description: 前端/Web开发相关——HTML/CSS/JS、React框架、响应式设计、性能优化、无障碍
---

# 前端开发 (Web Development)

前端/Web 开发相关——HTML/CSS/JS、React 框架、响应式设计、性能优化、无障碍。

## 技术栈

- **框架** — React/Next.js 为主要选型
- **样式** — Tailwind CSS / CSS Modules，避免全局样式污染
- **状态管理** — React Context + useReducer，需要跨页面共享时才引入 zustand/jotai
- **构建** — Vite（现代项目）或 Next.js 内置

## 性能检查清单

- 图片懒加载 (`loading="lazy"`) + 合适的尺寸格式 (WebP/AVIF)
- 大列表用虚拟滚动（`react-virtuoso`），不一次性渲染 1000+ 条
- API 请求加 debounce/throttle，搜索场景尤甚
- 避免 `useEffect` 中的同步请求链——聚合或并发
- React.memo / useMemo / useCallback 只在可测量性能问题时使用

## 设计原则

- 移动端优先 (mobile-first)，先适配小屏再加断点
- 可访问性不是可选项（aria-label、keyboard nav、color contrast）
- 用户偏好（暗色模式、减少动画）用 `prefers-*` media query
- 不使用 div 模拟 button——语义 HTML 优先
