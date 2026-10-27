//! Interface strings. The English text is the key and the source of truth;
//! [`tr`] returns the Chinese text for a key, or the key itself when there is
//! no translation. Dynamic messages use `{}` placeholders, filled in order by
//! [`fill`]. Every translation must keep the same number of placeholders in
//! the same order (checked by the unit tests).

use std::{
  collections::HashMap,
  fmt::{Display, Write as _},
  sync::OnceLock,
};

use crate::language::Language;

const ZH: &[(&str, &str)] = &[
  // Title, theme and game selector.
  ("MH Save Editor", "MH 存档编辑器"),
  ("Theme: {}", "主题: {}"),
  ("Dark", "深色"),
  ("Light", "浅色"),
  ("✘ {} saves are not supported yet.", "✘ {} 存档暂不支持。"),
  ("Editable", "可编辑"),
  ("Not supported yet", "暂不支持"),
  // Save selection and tabs.
  ("Quick select", "快速选择"),
  ("(no saves found)", "（未找到存档）"),
  ("Target save", "目标存档"),
  ("Browse…", "浏览…"),
  ("Choose target save", "选择目标存档"),
  ("Choose source save", "选择源存档"),
  ("MH Rise save", "MH Rise 存档"),
  ("Open save (unsaved changes)", "打开存档（有未保存的更改）"),
  ("Open save", "打开存档"),
  ("✘ Last operation failed: {}", "✘ 上一次操作失败: {}"),
  ("Opened: {} ({}){}", "已打开: {} ({}){}"),
  (" · ● unsaved changes", " · ● 有未保存的更改"),
  ("No save open", "未打开存档"),
  ("General", "概览"),
  ("Player", "玩家"),
  ("Item Box", "道具箱"),
  ("Characters", "角色"),
  ("Items", "道具"),
  ("Item Pouch", "道具袋"),
  ("Ammo Pouch", "弹药袋"),
  ("Ammo Box", "弹药箱"),
  ("Material Box", "素材箱"),
  ("Deco Box", "装饰品箱"),
  ("Search", "搜索"),
  ("Add", "添加"),
  ("Bulk edit", "批量编辑"),
  ("Loading…", "加载中…"),
  ("Explorer", "浏览器"),
  ("Decode", "解析"),
  ("Encode", "写回"),
  ("Save contents", "存档内容"),
  ("Refresh", "刷新"),
  ("! Write or discard your changes before switching games", "! 切换游戏前请先写入或放弃更改"),
  ("Loading {} saves", "正在加载{}存档"),
  ("{} of {} shown", "显示 {} / {}"),
  ("● Unsaved", "● 未保存"),
  ("Close the game first.", "写入前请先关闭游戏。"),
  ("ID", "ID"),
  ("HR", "HR"),
  ("HR XP", "HR 经验"),
  ("MR", "MR"),
  ("MR XP", "MR 经验"),
  ("Research", "研究点"),
  ("Loadouts", "装备方案"),
  ("Transfer", "转移"),
  ("Progress", "进度"),
  ("Backup", "备份"),
  ("Help", "帮助"),
  ("(unknown)", "（未知）"),
  ("{} · Curve {}", "{} · 曲线 {}"),
  ("Activity log", "活动日志"),
  ("(ready)", "（就绪）"),
  ("Source save", "源存档"),
  ("Source: {} ({})", "源存档: {} ({})"),
  ("Source quick select", "源存档快速选择"),
  ("(none selected)", "（未选择）"),
  ("(no need to transfer if same as target)", "（与目标相同则无需转移）"),
  ("Open source save", "打开源存档"),
  ("Unload source save", "卸载源存档"),
  ("Source save unloaded", "源存档已卸载"),
  ("Save as new file", "另存为新文件"),
  ("Write to save (backs up first)", "写入存档（先备份）"),
  ("● Unsaved changes (in memory)", "● 有未保存的更改（仅在内存中）"),
  ("● Unsaved changes", "● 有未保存的更改"),
  ("Close the game before writing; Write to save backs up the whole save folder to Documents\\MHR-Save-Backups first", "写入前请先关闭游戏；“写入存档”会先把整个存档文件夹备份到 Documents\\MHR-Save-Backups"),
  ("SteamID64 filled in from the save path: {}", "已根据存档路径填入 SteamID64: {}"),
  // Activity log messages.
  ("✔ Opened {}: {} ({})", "✔ 已打开{}: {} ({})"),
  ("source save", "源存档"),
  ("target save", "目标存档"),
  ("✘ Open failed: {}", "✘ 打开失败: {}"),
  ("! SteamID64 corrected to {} from the save path; try again", "! 已根据存档路径将 SteamID64 更正为 {}，请重试"),
  ("✔ Written: {}{}", "✔ 已写入: {}{}"),
  (" (backed up to {})", "（已备份到 {}）"),
  ("✘ Save failed: {}", "✘ 保存失败: {}"),
  ("✘ Worker thread exited unexpectedly", "✘ 后台线程意外退出"),
  ("▶ Restoring {} …", "▶ 正在还原 {} …"),
  ("! Unsaved changes: choose the save again to discard them and reopen", "! 有未保存的更改：再次选择该存档即可放弃更改并重新打开"),
  ("✘ SteamID64 must be a number", "✘ SteamID64 必须是数字"),
  ("Choose a save", "选择存档"),
  ("! Restore overwrites every file in the save folder; click Restore this backup again to confirm", "! 还原会覆盖存档文件夹中的所有文件；请再次点击“还原此备份”以确认"),
  ("✔ Changes applied (in memory)", "✔ 更改已应用（仅在内存中）"),
  ("✔ {} (in memory)", "✔ {}（仅在内存中）"),
  ("✔ Item changes applied: {} (in memory)", "✔ 道具更改已应用: {}（仅在内存中）"),
  ("Open source save: {}", "打开源存档: {}"),
  ("Open save: {}", "打开存档: {}"),
  ("Save to: {}", "保存到: {}"),
  ("Write to original save (backup first: {})", "写入原存档（先备份: {}）"),
  ("Back up folder: {}", "备份文件夹: {}"),
  ("Restore {} to {}", "将 {} 还原到 {}"),
  ("Backed up {} files ({} bytes) to {}; every copy verified byte by byte", "已备份 {} 个文件（{} 字节）到 {}；每个副本均已逐字节校验"),
  ("Restored {} files ({} bytes) to {}; reopen the save to see it", "已还原 {} 个文件（{} 字节）到 {}；请重新打开存档查看"),
  // Shared errors.
  ("No target save is open", "未打开目标存档"),
  ("No source save is open", "未打开源存档"),
  ("Target slot must be a number", "目标格子必须是数字"),
  ("Source slot must be a number", "源格子必须是数字"),
  ("SteamID64 must be digits only", "SteamID64 只能包含数字"),
  // General tab.
  ("No save open. Choose one with Quick select or Browse above.", "未打开存档。请通过上方的快速选择或浏览选择一个存档。"),
  ("Hunter name", "猎人名称"),
  ("(not found)", "（未找到）"),
  ("Curve Index", "曲线索引"),
  ("Decrypted size", "解密后大小"),
  ("{} bytes", "{} 字节"),
  ("Container: DSSS v{}, flags=0x{}, file {} bytes", "容器: DSSS v{}, flags=0x{}, 文件 {} 字节"),
  ("{} slots, {} non-empty", "{} 个格子，{} 个非空"),
  ("Zenny / Points", "金币 / 点数"),
  ("{} / {} ({} earned in total)", "{} / {}（累计获得 {}）"),
  ("Rank (HR / MR / Anomaly)", "等级 (HR / MR / 异常调查)"),
  ("File", "文件"),
  // Player tab.
  ("No save open.", "未打开存档。"),
  ("Zenny", "金币"),
  ("Total Zenny earned", "累计获得金币"),
  ("Points", "点数"),
  ("Hunter Rank (HR)", "猎人等级 (HR)"),
  ("Master Rank (MR)", "大师等级 (MR)"),
  ("Anomaly Research Rank", "异常调查等级"),
  ("Apply changes", "应用更改"),
  ("(leave blank = no change; the name is read-only)", "（留空 = 不变；名称为只读）"),
  ("! Ranks are limited by the unlock system: if the matching unlock quests (Hub / Master Rank / Anomaly Investigation urgent chains) are not complete, the game shows an achievement popup on load but still clamps the rank to the current cap. Unlock state is stored in a quest-clear bitmap that this tool cannot rewrite yet; finish the unlock quests in game, or use Transfer to move over a high-rank save.", "! 等级受解锁系统限制：如果对应的解锁任务（村庄 / 大师等级 / 异常调查紧急任务链）尚未完成，读取时游戏会弹出成就提示，但仍会把等级限制在当前上限。解锁状态保存在任务通关位图中，本工具暂时无法改写；请在游戏中完成解锁任务，或使用转移功能迁移高等级存档。"),
  ("Hunter Rank", "猎人等级"),
  ("Master Rank", "大师等级"),
  ("Money field not found (schema mismatch)", "未找到金币字段（存档结构不匹配）"),
  ("Points field not found (schema mismatch)", "未找到点数字段（存档结构不匹配）"),
  ("Rank must be a number: {}", "等级必须是数字: {}"),
  ("{} out of range (1-{}): {}", "{} 超出范围 (1-{}): {}"),
  ("Rank field not found (schema mismatch)", "未找到等级字段（存档结构不匹配）"),
  ("Nothing to apply (values unchanged or empty)", "没有可应用的更改（数值未变化或为空）"),
  // Item Box tab.
  ("Search name / ID", "搜索名称 / ID"),
  ("All categories", "全部分类"),
  ("Consumables", "消耗品"),
  ("Materials", "素材"),
  ("Ammo / Bottles", "弹药 / 瓶"),
  ("Sell / Antiques", "售卖 / 古物"),
  ("Tools / Other", "工具 / 其他"),
  ("Show empty slots", "显示空格子"),
  ("Apply item changes ({})", "应用道具更改（{}）"),
  ("(loading…)", "（加载中…）"),
  ("Edit selected", "编辑所选"),
  ("{} selected", "已选 {} 项"),
  ("Select all shown", "全选当前显示"),
  ("Clear selection", "清除选择"),
  ("Quantity", "数量"),
  ("5, +5 or -5", "5、+5 或 -5"),
  ("Apply", "应用"),
  ("Delete selected", "删除所选"),
  ("Move selection to slot", "将所选移动到格子"),
  ("Move", "移动"),
  ("Sort box by", "道具箱排序方式"),
  ("Name", "名称"),
  ("Category", "分类"),
  ("Quantity (high to low)", "数量（从高到低）"),
  ("Sort (empty slots go last)", "排序（空格子放最后）"),
  ("Slot", "格子"),
  ("Qty", "数量"),
  ("New qty", "新数量"),
  ("(empty slot)", "（空格子）"),
  ("Showing {} of {} slots. Tick rows (shift-click for a range) to edit many at once. Per row, New qty accepts 5 (set), +5 (add) or -5 (subtract); blank = no change, 0 = clear the item.", "显示 {} / {} 个格子。勾选行（按住 Shift 点击可选择范围）即可批量编辑。每行的新数量可输入 5（设为）、+5（增加）或 -5（减少）；留空 = 不变，0 = 清空该道具。"),
  ("{} backups found", "已找到 {} 个备份"),
  ("Deleted {} slots", "已删除 {} 个格子"),
  ("Set quantity to {}", "数量设为 {}"),
  ("Added {} to the quantity", "数量增加 {}"),
  ("Subtracted {} from the quantity", "数量减少 {}"),
  ("{} on {} slots", "{}，共 {} 个格子"),
  ("Slots start at 1", "格子编号从 1 开始"),
  ("Moved {} slots to start at slot {}", "已移动 {} 个格子，从第 {} 格开始"),
  ("Item Box sorted by {}", "道具箱已按{}排序"),
  ("{} updated, {} new items placed in empty slots", "已更新 {} 项，{} 个新道具放入空格子"),
  ("Nothing to apply (quantities unchanged or empty)", "没有可应用的更改（数量未变化或为空）"),
  // Loadouts tab.
  ("Open the target save first (Quick select or Browse above).", "请先打开目标存档（上方快速选择或浏览）。"),
  ("Target loadouts (224 slots)", "目标装备方案（224 个格子）"),
  ("{} registered; the Equipment Box slots a loadout references can be moved with it", "已登记 {} 个；装备方案引用的装备箱格子会随之移动"),
  ("(empty)", "（空）"),
  ("Transfer from source save", "从源存档转移"),
  ("Open a source save on the Transfer tab first to transfer loadouts or single pieces.", "请先在“转移”页打开源存档，再转移装备方案或单件装备。"),
  ("The source save has no registered loadouts.", "源存档没有已登记的装备方案。"),
  ("Source loadout:", "源装备方案:"),
  ("Referenced gear:", "引用的装备:"),
  ("Also move the gear the loadout references (overwrites the same slots)", "同时移动装备方案引用的装备（覆盖相同的格子）"),
  ("Target slot (blank = first empty slot):", "目标格子（留空 = 第一个空格子）:"),
  ("Transfer loadout", "转移装备方案"),
  ("Single piece transfer", "单件装备转移"),
  ("Source slot:", "源格子:"),
  ("Target slot:", "目标格子:"),
  ("Transfer piece (overwrites target slot)", "转移单件装备（覆盖目标格子）"),
  ("Gear is addressed by Equipment Box slot number; make sure the target slot contents are no longer needed.", "装备按装备箱格子编号指定；请确认目标格子中的内容已不再需要。"),
  ("Every transfer applies in memory at once. When it looks right, write to disk with the buttons below (writing to the original save backs up first).", "每次转移都会立即在内存中生效。确认无误后，用下方按钮写入磁盘（写入原存档会先备份）。"),
  ("Loadout transferred to slot {}{}", "装备方案已转移到格子 {}{}"),
  (", referenced gear moved to the same slots", "，引用的装备已移到相同格子"),
  ("Gear moved from source slot {} to target slot {}", "装备已从源格子 {} 移到目标格子 {}"),
  // Transfer tab.
  ("Open the target save first (Quick select or Browse above) before transferring.", "转移前请先通过上方的快速选择或浏览打开目标存档。"),
  ("Merge data from the source save into the target save (same Steam account):", "将源存档的数据合并到目标存档（同一 Steam 账号）:"),
  ("Item Box transfer", "道具箱转移"),
  ("Categories taken from source:", "从源存档取用的分类:"),
  ("Select all", "全选"),
  ("Clear", "清空"),
  ("Transfer Item Box (merge by category)", "转移道具箱（按分类合并）"),
  ("Only slots in ticked categories are taken from the source; the rest stay as in the target. All ticked = full replacement", "只从源存档取用勾选分类中的格子，其余保持目标存档不变。全部勾选 = 完全替换"),
  ("Item Box merged: of {} slots, {} items from source, {} kept from target (source has {} items)", "道具箱已合并: 共 {} 个格子，从源存档转入 {} 项，保留目标存档 {} 项（源存档共有 {} 项）"),
  ("Progress transfer", "进度转移"),
  ("Rank progress (HR / MR / Anomaly Research rank + points)", "等级进度（HR / MR / 异常调查等级 + 点数）"),
  ("Transfer rank progress", "转移等级进度"),
  ("! If the target has not finished the unlock quests, the game still clamps the transferred ranks to the current cap", "! 若目标存档尚未完成解锁任务，游戏仍会把转移过来的等级限制在当前上限内"),
  ("Rank progress transferred; target is now HR {} / MR {} / Anomaly {}", "等级进度已转移；目标存档现为 HR {} / MR {} / 异常调查 {}"),
  ("Equipment transfer", "装备转移"),
  ("Equipment Box (weapons / armor / talismans)", "装备箱（武器 / 防具 / 护石）"),
  ("Worn gear and loadout registry", "身上装备与装备方案登记"),
  ("Item pouch loadouts", "道具袋装备方案"),
  ("Transfer equipment (selected parts)", "转移装备（所选部分）"),
  ("! In a partial transfer, Equipment Box indexes referenced by loadouts or worn gear may point to different gear; check in game", "! 部分转移时，装备方案或身上装备引用的装备箱编号可能指向不同的装备；请在游戏中检查"),
  ("Transfers apply in memory at once; when it looks right, write to disk with Save as new file below.", "转移会立即在内存中生效；确认无误后，用下方的“另存为新文件”写入磁盘。"),
  ("Equipment transferred: {}; Equipment Box {} slots, {} items{}", "装备已转移: {}；装备箱 {} 个格子，{} 件装备{}"),
  ("{}={} fields", "{}={} 个字段"),
  (" (partial transfer: check worn gear and loadouts in game)", "（部分转移：请在游戏中检查身上装备和装备方案）"),
  ("✘ Output path equals the source file; the source save is never modified", "✘ 输出路径与源文件相同；源存档绝不会被修改"),
  // Progress tab.
  ("Open a save first (Quick select or Browse above).", "请先打开存档（上方快速选择或浏览）。"),
  ("Ranks and accumulated points", "等级与累计点数"),
  ("Points are the running progress toward the next rank; once the clamp is lifted the game advances the rank from them. Blank = no change.", "点数是通往下一等级的累计进度；解除上限限制后，游戏会根据点数提升等级。留空 = 不变。"),
  ("Hunter Rank points (HR)", "猎人等级点数 (HR)"),
  ("Master Rank points (MR)", "大师等级点数 (MR)"),
  ("Anomaly Research points", "异常调查点数"),
  ("Apply point changes", "应用点数更改"),
  ("(points are only accumulated progress and do not change the unlock clamp; 1 ≤ value ≤ 99999999)", "（点数只是累计进度，不会改变解锁上限；1 ≤ 数值 ≤ 99999999）"),
  ("Hunter Rank points", "猎人等级点数"),
  ("Master Rank points", "大师等级点数"),
  ("{} must be a number: {}", "{}必须是数字: {}"),
  ("{} out of range (1-99999999): {}", "{} 超出范围 (1-99999999): {}"),
  ("Nothing to apply", "没有可应用的更改"),
  ("Updated {} point values", "已更新 {} 个点数"),
  ("Guild card unlock flags (display copy)", "公会卡解锁标记（显示副本）"),
  ("Apply card flags", "应用公会卡标记"),
  ("These are display copies on the hunter card; in testing the unlock clamp is not fully decided by them", "这些是猎人卡上的显示副本；测试表明解锁上限并不完全由它们决定"),
  ("Updated {} card flags", "已更新 {} 个公会卡标记"),
  ("Master Rank unlocked (isMRRelease)", "大师等级已解锁 (isMRRelease)"),
  ("Show Master Rank (isMRDisp)", "显示大师等级 (isMRDisp)"),
  ("Anomaly Investigations unlocked", "异常调查已解锁"),
  ("Special Anomaly Investigations unlocked", "特殊异常调查已解锁"),
  ("Endgame content unlocked", "终局内容已解锁"),
  ("Buddy quests unlocked", "伙伴任务已解锁"),
  ("Buddy secrets unlocked", "伙伴秘密已解锁"),
  ("Custom weapon editing unlocked", "自定义武器编辑已解锁"),
  ("Why does my rank change not stick?", "为什么我修改的等级没有生效？"),
  ("· The game clamps ranks: until the matching unlock quest chain is done, loading shows an achievement popup but the displayed rank is pushed back to the current cap", "· 游戏会限制等级：在对应的解锁任务链完成之前，读取存档时会弹出成就提示，但显示的等级会被压回当前上限"),
  ("· Unlock state lives in a packed quest-clear bitmap (mapping quests to bits needs the game data tables), which this tool does not rewrite yet", "· 解锁状态保存在压缩的任务通关位图中（把任务映射到位需要游戏数据表），本工具暂不会改写它"),
  ("· To get a new save to a high rank: finish the unlock quests in game, or use Transfer to move gear / items / rank progress from a high-rank save (unlocks are still bound by quest progress)", "· 要让新存档达到高等级：请在游戏中完成解锁任务，或使用转移功能从高等级存档迁移装备 / 道具 / 等级进度（解锁仍受任务进度限制）"),
  // Backup tab.
  ("Open a save first (Quick select or Browse above) so its save folder can be located.", "请先打开存档（上方快速选择或浏览），以便定位其存档文件夹。"),
  ("Current save folder: {}", "当前存档文件夹: {}"),
  ("A backup copies the whole save folder as-is to Documents\\MHR-Save-Backups\\<account>\\<timestamp>, verified byte by byte and outside Steam Cloud sync;", "备份会将整个存档文件夹原样复制到 Documents\\MHR-Save-Backups\\<账号>\\<时间戳>，逐字节校验，且不参与 Steam 云同步；"),
  ("Restore copies the chosen backup over the save folder. Close the game first and do not restore while Steam Cloud is syncing.", "还原会把所选备份覆盖到存档文件夹。请先关闭游戏，并且不要在 Steam 云同步进行时还原。"),
  ("Back up whole save folder", "备份整个存档文件夹"),
  ("Existing backups (newest first)", "现有备份（最新在前）"),
  ("(no backups yet; back up once before your first edit)", "（还没有备份；首次编辑前请先备份一次）"),
  ("! Click again to confirm restore", "! 再次点击以确认还原"),
  ("Restore this backup", "还原此备份"),
  ("Cancel", "取消"),
  // Help tab.
  ("Save folder: <Steam library>\\userdata\\<32-bit account id>\\1446780\\remote\\win64_save", "存档文件夹: <Steam 库>\\userdata\\<32 位账号 ID>\\1446780\\remote\\win64_save"),
  ("· Steam installs to C:\\Program Files (x86)\\Steam by default; custom libraries may be on other drives (for example D:\\Steam). All Steam libraries are scanned at startup.", "· Steam 默认安装在 C:\\Program Files (x86)\\Steam；自定义的库可能位于其他盘符（例如 D:\\Steam）。启动时会扫描所有 Steam 库。"),
  ("· Character saves: data001Slot.bin ~ data003Slot.bin (up to 3 character slots in game)", "· 角色存档: data001Slot.bin ~ data003Slot.bin（游戏中最多 3 个角色栏位）"),
  ("· System save: data00-1.bin (global data, normally not edited); SS1_* / SS4_* / SS7_* are album data", "· 系统存档: data00-1.bin（全局数据，通常不需要编辑）；SS1_* / SS4_* / SS7_* 是图鉴数据"),
  ("· Recommended flow: back up → edit → write to save (backs up the whole folder first) → check in game. Save as new file writes <name>.edited.bin that you must swap in by hand", "· 推荐流程: 备份 → 编辑 → 写入存档（会先备份整个文件夹）→ 在游戏中检查。另存为新文件会生成 <名称>.edited.bin，需要你手动替换"),
  ("· Steam Cloud: go offline in Steam while editing so an older cloud save cannot overwrite it", "· Steam 云存档: 编辑时请在 Steam 中切换为离线，以免旧的云存档覆盖你的修改"),
  ("· The character save must already exist (the game will not recognise a file dropped into an empty slot)", "· 角色存档必须已经存在（游戏不会识别放进空栏位的文件）"),
  ("Cannot type Chinese / no candidate window?", "无法输入中文 / 没有候选词窗口？"),
  ("· This app uses the winit window library, which has a known compatibility problem with TSF-mode input methods (keys swallowed, no candidate list)", "· 本程序使用 winit 窗口库，该库与 TSF 模式的输入法存在已知的兼容性问题（按键被吞、没有候选词列表）"),
  ("· Microsoft Pinyin: Settings → Time & language → Language & region → Chinese → Microsoft Pinyin → General → Compatibility → turn on 'Use previous version of Microsoft IME'", "· 微软拼音: 设置 → 时间和语言 → 语言和区域 → 中文 → 微软拼音 → 常规 → 兼容性 → 开启“使用旧版 Microsoft IME”"),
  ("· Sogou and other third-party IMEs: if they fail too, it is the same compatibility problem; switch to Microsoft Pinyin in compatibility mode. This affects every winit app and has nothing to do with this tool", "· 搜狗等第三方输入法：如果同样失败，也是同一兼容性问题；请切换到开启兼容模式的微软拼音。这影响所有 winit 程序，与本工具无关"),
];

fn zh_table() -> &'static HashMap<&'static str, &'static str> {
  static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
  TABLE.get_or_init(|| ZH.iter().copied().collect())
}

/// The text to show for `english` in `language`; the English text itself when
/// there is no translation.
pub fn tr(language: Language, english: &'static str) -> &'static str {
  match language {
    Language::English => english,
    Language::Chinese => zh_table().get(english).copied().unwrap_or(english),
  }
}

/// Fills the `{}` placeholders of a translated template, in order.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
  let mut out = String::with_capacity(template.len() + 16);
  let mut pieces = template.split("{}");
  if let Some(first) = pieces.next() {
    out.push_str(first);
  }
  for (arg, piece) in args.iter().zip(pieces) {
    let _ = write!(out, "{arg}{piece}");
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn keys_are_unique_and_translated() {
    let mut seen = std::collections::HashSet::new();
    for (english, chinese) in ZH {
      assert!(seen.insert(*english), "duplicate key: {english}");
      assert!(!chinese.trim().is_empty(), "empty translation for: {english}");
    }
  }

  #[test]
  fn translations_keep_placeholders() {
    for (english, chinese) in ZH {
      assert_eq!(
        english.matches("{}").count(),
        chinese.matches("{}").count(),
        "placeholder count differs for: {english}"
      );
    }
  }

  #[test]
  fn tr_falls_back_to_english() {
    assert_eq!(tr(Language::English, "Apply"), "Apply");
    assert_eq!(tr(Language::Chinese, "Apply"), "应用");
    assert_eq!(tr(Language::Chinese, "no such key"), "no such key");
  }

  #[test]
  fn fill_substitutes_in_order() {
    assert_eq!(fill("{} · Curve {}", &[&"Hunter", &7]), "Hunter · Curve 7");
    assert_eq!(fill("no placeholders", &[]), "no placeholders");
  }
}
