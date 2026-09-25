/*
 * Z·POOL 自研图标集。
 *
 * 20×20 网格、1.6 描边、圆头圆角（round cap/join），接近 SF Symbols 的观感。
 * 全部跟随 currentColor，放进任何配色的按钮/文字里都会自动变色。
 */

const ICONS = {
  // 邮箱（导航 / 输入弹窗）
  mail: `<rect x="2.7" y="4.8" width="14.6" height="10.4" rx="2.6"/>
         <path d="M4 7l6 4.2L16 7"/>`,

  // 账号（导航）
  person: `<circle cx="10" cy="6.8" r="3.1"/>
           <path d="M4.2 16.6c0-3 2.6-4.9 5.8-4.9s5.8 1.9 5.8 4.9"/>`,

  // 设置（滑杆风）
  sliders: `<path d="M3 6.5h6"/><path d="M14 6.5h3"/><circle cx="11.5" cy="6.5" r="2.2"/>
            <path d="M3 13.5h3"/><path d="M11 13.5h6"/><circle cx="8.5" cy="13.5" r="2.2"/>`,

  // 导入（落进托盘）
  import: `<path d="M10 3.4v8.2"/><path d="M6.6 8.4 10 11.8l3.4-3.4"/><path d="M4 16.6h12"/>`,
  // 导出（冲出托盘）
  export: `<path d="M10 12.6V4.4"/><path d="M6.6 7.8 10 4.4l3.4 3.4"/><path d="M4 16.6h12"/>`,
  // 导出全部
  exportAll: `<path d="M4 16.6h12"/><path d="M6.6 13.4h6.8"/><path d="M10 3.4v6"/><path d="M7.2 6 10 3.2 12.8 6"/>`,
  // 保存当前登录
  capture: `<path d="M10 3.4v8.2"/><path d="M6.6 8.4 10 11.8l3.4-3.4"/><path d="M4 16.6h12"/>`,

  // 播放
  play: `<path d="M7 5.4 15 10l-8 4.6z" fill="currentColor" stroke="none"/>`,
  // 停止 / 电源
  power: `<path d="M10 3.2v7.6"/><path d="M6.2 6.4a6 6 0 1 0 7.6 0"/>`,
  x: `<path d="M6.2 6.2l7.6 7.6"/><path d="M13.8 6.2l-7.6 7.6"/>`,
  check: `<path d="M5 10.6l3.4 3.4L15 6.6"/>`,

  // 切换（来回箭头）
  swap: `<path d="M4 7h10"/><path d="M11.2 4.2 14 7l-2.8 2.8"/>
         <path d="M16 13H6"/><path d="M8.8 10.2 6 13l2.8 2.8"/>`,
  // 刷新
  refresh: `<path d="M16 10a6 6 0 1 1-1.8-4.3"/><path d="M16 3.6V7h-3.4"/>`,

  // 额度（速度表）
  gauge: `<path d="M3.2 14a6.8 6.8 0 0 1 13.6 0"/><path d="M10 14l3-4"/>`,
  // 重命名
  pen: `<path d="M4 16l.8-3.2 8-8 2.4 2.4-8 8z"/><path d="M11.6 5.6l2.4 2.4"/>`,
  // 领取（礼盒）
  gift: `<rect x="3.6" y="8.4" width="12.8" height="7.6" rx="1.8"/><path d="M3.6 11.6h12.8"/><path d="M10 8.4v7.6"/>
         <path d="M10 8.4c-1.4-2.8-4.6-2.8-4.6-.9 0 1.1 1.8 1 4.6.9z"/>
         <path d="M10 8.4c1.4-2.8 4.6-2.8 4.6-.9 0 1.1-1.8 1-4.6.9z"/>`,

  // 新增账号
  userPlus: `<circle cx="8" cy="6.8" r="3.1"/><path d="M3 16.6c0-2.9 2.3-4.7 5-4.7.9 0 1.8.2 2.5.6"/>
             <path d="M15.2 10.8v5"/><path d="M12.7 13.3h5"/>`,

  // 锁定
  lock: `<rect x="4.6" y="9" width="10.8" height="7.4" rx="2"/><path d="M6.9 9V6.6a3.1 3.1 0 0 1 6.2 0V9"/>`,
  lockOpen: `<rect x="4.6" y="9" width="10.8" height="7.4" rx="2"/><path d="M6.9 9V6.6a3.1 3.1 0 0 1 6-1.1"/>`,

  // 警告
  alert: `<path d="M10 3.6 17.4 16.4H2.6z"/><path d="M10 8.4v3.4"/><path d="M10 14.1v.1"/>`,

  // 空状态
  empty: `<rect x="3.6" y="3.6" width="12.8" height="12.8" rx="3.4"/><path d="M10 7.4v5.2"/><path d="M7.4 10h5.2"/>`,

  // 删除
  trash: `<path d="M5 6h10"/><path d="M8.2 6V4.6a1 1 0 0 1 1-1h1.6a1 1 0 0 1 1 1V6"/>
          <path d="M6.6 6l.7 9a1.6 1.6 0 0 0 1.6 1.5h2.2a1.6 1.6 0 0 0 1.6-1.5l.7-9"/>`,
};

export function ic(name, size = 16, cls = "") {
  const body = ICONS[name];
  if (!body) return "";
  return `<svg class="ic${cls ? " " + cls : ""}" width="${size}" height="${size}" viewBox="0 0 20 20"
    fill="none" stroke="currentColor" stroke-width="1.6"
    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
}
