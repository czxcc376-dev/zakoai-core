def read(p): return open(p, encoding='utf-8').read()
def write(p, s):
    open(p, 'w', encoding='utf-8').write(s)
    print('wrote', p, len(s))

def sub(s, old, new, tag):
    assert old in s, f'{tag}: anchor not found'
    return s.replace(old, new, 1)

# ---------------------------------------------------------------- api.ts
p = 'web/src/api.ts'
s = read(p)
s = sub(s, """  request_timeout_seconds: number;
  retention_days: number;
};
export type Release = {""",
"""  request_timeout_seconds: number;
  retention_days: number;
  /** always | auto | off */
  inject_mode: string;
  inject_off_below: number;
  inject_full_from: number;
  inject_markers: boolean;
};
export type Release = {""", 'api.ts Settings')

s = sub(s, """  metadata?: {
    source_url: string;
    source_revision: string;
    variant: string;
    validation: string;
  };
};""",
"""  metadata?: {
    source_url: string;
    source_revision: string;
    variant: string;
    validation: string;
  };
  /** 实测探针结果，来自 docs/scores.json；未测过则为空 */
  score?: {
    passed: number;
    total: number;
    tested_at?: string;
    models?: Record<string, { passed: number; total: number }>;
  };
};""", 'api.ts Profile')
write(p, s)

# ---------------------------------------------------------------- Settings.tsx
p = 'web/src/Settings.tsx'
s = read(p)

s = sub(s, "  const [form, setForm] = useState<Settings>(state.settings);",
"""  const [form, setForm] = useState<Settings>({
    ...state.settings,
    inject_mode: state.settings.inject_mode ?? "always",
    inject_off_below: state.settings.inject_off_below ?? 2,
    inject_full_from: state.settings.inject_full_from ?? 8,
    inject_markers: state.settings.inject_markers ?? false,
  });""", 'Settings.tsx initializer')

anchor = """                    <span>天<\/span>
                  <\/span>
                <\/Field>
              <\/div>
            <\/div>
          <\/section>"""
insert = """                    <span>天<\/span>
                  <\/span>
                <\/Field>
              <\/div>
              <div className="default-strategy-setting">
                <strong>注入策略<\/strong>
                <p className="field-hint">
                  默认 always 与旧版完全一致；auto 会按用户请求里的关键词得分，决定「注入全文 \/ 只注身份帧 \/ 不注入」。
                <\/p>
                <div className="form-grid three">
                  <Field label="注入模式">
                    <select
                      value={form.inject_mode}
                      onChange={(event) => update("inject_mode", event.target.value)}
                    >
                      <option value="always">always · 全量注入（默认）<\/option>
                      <option value="auto">auto · 按需投影<\/option>
                      <option value="off">off · 不注入<\/option>
                    <\/select>
                  <\/Field>
                  <Field label="不注入阈值" hint="auto 模式下得分低于此值就不注入">
                    <span className="suffixed-input">
                      <input
                        type="number"
                        value={form.inject_off_below}
                        onChange={(event) =>
                          update("inject_off_below", Number(event.target.value))
                        }
                        min={0}
                        max={99}
                        required
                      \/>
                      <span>分<\/span>
                    <\/span>
                  <\/Field>
                  <Field label="全文注入阈值" hint="达到此值注入完整正文，之间只注身份帧">
                    <span className="suffixed-input">
                      <input
                        type="number"
                        value={form.inject_full_from}
                        onChange={(event) =>
                          update("inject_full_from", Number(event.target.value))
                        }
                        min={1}
                        max={99}
                        required
                      \/>
                      <span>分<\/span>
                    <\/span>
                  <\/Field>
                <\/div>
                <label className="toggle-label">
                  <input
                    type="checkbox"
                    checked={form.inject_markers}
                    onChange={(event) => update("inject_markers", event.target.checked)}
                  \/>
                  <span>用标记块包裹注入内容（auto 模式强制开启，保证多轮不叠加）<\/span>
                <\/label>
              <\/div>
            <\/div>
          <\/section>"""
s = sub(s, anchor, insert, 'Settings.tsx section')
write(p, s)

# ---------------------------------------------------------------- Management.tsx
p = 'web/src/Management.tsx'
s = read(p)
s = sub(s, """                      {item.metadata?.validation === "unverified" && (
                        <span className="profile-validation">未实测<\/span>
                      )}""",
"""                      {item.score ? (
                        <span
                          className="profile-validation verified"
                          title={`实测 ${item.score.passed}\/${item.score.total}${
                            item.score.tested_at ? ` · ${item.score.tested_at}` : ""
                          }`}
                        >
                          实测 {item.score.passed}\/{item.score.total}
                        <\/span>
                      ) : (
                        item.metadata?.validation === "unverified" && (
                          <span className="profile-validation">未实测<\/span>
                        )
                      )}""", 'Management.tsx badge')
write(p, s)

# ---------------------------------------------------------------- style.css
p = 'web/src/style.css'
s = read(p)
s = sub(s, ".profile-validation { color: var(--ink); }",
".profile-validation { color: var(--ink); }\n.profile-validation.verified { color: #2f7d5f; font-weight: 600; }", 'style.css')
write(p, s)

print('frontend patched')
