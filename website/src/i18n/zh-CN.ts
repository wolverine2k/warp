export const zhCN = {
  meta: {
    title: "OpenWarp — 为 Warp 解锁自定义 AI 提供商",
    description:
      "OpenWarp 是 Warp 的开放式增强项目。通过 genai 适配层原生接入 OpenAI / Anthropic / Gemini / DeepSeek / Ollama 等多协议提供商,自定义系统提示词,享受真正属于你的智能终端。",
  },
  nav: {
    how: "工作方式",
    features: "特性",
    providers: "自定义提供商",
    faq: "FAQ",
    docs: "文档",
    github: "GitHub",
  },
  hero: {
    badge: "社区版 · 进行中",
    title_1: "把",
    title_em: "任意 AI 模型",
    title_2: "接入你的终端",
    subtitle:
      "OpenWarp 是 Warp 的开源增强版,引入 BYOP(自带提供商)层,通过 genai 原生支持 6 种 API 协议,自定义模型、提示词与界面语言。",
    cta_primary: "查看 GitHub",
    cta_secondary: "阅读文档",
    note: "当前项目处于早期开发,尚未发布正式版本",
    trust_lead: "兼容主流提供商",
    showcase: {
      connected: "● 已连接 · genai",
      local_only: "仅本地",
      template: "user.role / locale 上下文渲染",
    },
  },
  stats: {
    items: [
      { value: "∞", label: "可接入提供商" },
      { value: "2", label: "内置语言" },
      { value: "AGPL", label: "开源许可" },
      { value: "100%", label: "本地凭证存储" },
    ],
  },
  how: {
    eyebrow: "工作方式",
    title: "三步,接入你自己的 AI",
    subtitle:
      "保留 Warp 的全部交互,只在 AI 层完全开放:密钥、模型、提示词由你配置。",
    steps: [
      {
        num: "01",
        title: "接入任意提供商",
        desc: "在设置中选择 API 协议、填入 Base URL 与 API Key,OpenAI / Anthropic / Gemini / Ollama / DeepSeek 6 种协议任意切换,凭证仅保存在本地。",
      },
      {
        num: "02",
        title: "自定义系统提示词",
        desc: "minijinja 模板按当前目录、语言、角色实时渲染,把上下文交给模型。",
      },
      {
        num: "03",
        title: "在终端中即刻使用",
        desc: "切换模型、对话、命令补全的体验与 Warp 保持一致,但底层 AI 完全可控。",
      },
    ],
  },
  providers: {
    eyebrow: "自定义提供商",
    title: "一次配置,接入任意模型",
    subtitle:
      "OpenWarp 通过 genai 适配层原生支持 6 种 API 协议:OpenAI / OpenAI Responses / Anthropic / Gemini / Ollama / DeepSeek。协议显式指定,不依赖模型名识别,凭证与请求直连服务商,无中间转发。",
    fields: {
      name: "提供商名称",
      protocol: "API 协议",
      base_url: "Base URL",
      endpoint: "请求端点",
      api_key: "API Key",
      model: "默认模型",
      prompt: "系统提示词模板",
    },
    bullets: [
      "✓ 6 种 API 协议原生路由,不再以 OpenAI 兼容方式硬接",
      "✓ 推理内容多轮回传:DeepSeek reasoning_content / Claude thinking / Gemini",
      "✓ minijinja 模板按上下文渲染系统提示",
      "✓ 凭证仅保存在本地,请求直连服务商,无中间转发",
    ],
    tabs: [
      {
        id: "openai",
        name: "OpenAI",
        tag: "原生协议",
        protocol: "OpenAI",
        baseUrl: "https://api.openai.com",
        endpoint: "POST /v1/chat/completions",
        apiKey: "sk-•••••••••••••••••••••",
        model: "gpt-4o",
      },
      {
        id: "anthropic",
        name: "Anthropic",
        tag: "原生协议",
        protocol: "Anthropic",
        baseUrl: "https://api.anthropic.com",
        endpoint: "POST /v1/messages",
        apiKey: "sk-ant-•••••••••••••••••",
        model: "claude-sonnet-4-6",
      },
      {
        id: "gemini",
        name: "Gemini",
        tag: "原生协议",
        protocol: "Gemini",
        baseUrl: "https://generativelanguage.googleapis.com",
        endpoint: "POST /v1beta/models/{model}:generateContent",
        apiKey: "AIza•••••••••••••••••••",
        model: "gemini-2.0-flash",
      },
      {
        id: "deepseek",
        name: "DeepSeek",
        tag: "OpenAI 兼容",
        protocol: "OpenAI",
        baseUrl: "https://api.deepseek.com",
        endpoint: "POST /v1/chat/completions",
        apiKey: "sk-•••••••••••••••••••••",
        model: "deepseek-reasoner",
      },
      {
        id: "ollama",
        name: "Ollama",
        tag: "本地",
        protocol: "Ollama",
        baseUrl: "http://localhost:11434",
        endpoint: "POST /api/chat",
        apiKey: "— 不需要 —",
        model: "qwen2.5-coder:7b",
      },
    ],
  },
  features: {
    eyebrow: "核心特性",
    title: "AI、提示词、界面、协议 —— 全栈开放",
    items: [
      {
        title: "BYOP 自定义提供商",
        desc: "通过 genai 适配层原生支持 6 种 API 协议,Base URL / API Key / 模型自由组合。",
      },
      {
        title: "提示词模板",
        desc: "minijinja 模板按上下文动态渲染系统提示,精准引导模型。",
      },
      {
        title: "多语言界面",
        desc: "中文与英文一等公民,其他语种由社区扩展。",
      },
      {
        title: "隐私优先",
        desc: "Cloud Agent / Computer Use 默认关闭,不上传云端,凭证仅保存在本机。",
      },
      {
        title: "保留 Warp 体验",
        desc: "持续合并 Warp 上游,块、AI 命令、工作流、键位全部保留。",
      },
      {
        title: "开源协议",
        desc: "与 Warp 上游一致,采用 AGPL / MIT 双许可,代码全部公开。",
      },
      {
        title: "SSH 管理器",
        desc: "侧边栏树形分组管理远程主机,密钥与密码加密存入系统 keychain;单击即连,首次登录自动注入密码 / passphrase。",
      },
    ],
  },
  faq: {
    eyebrow: "常见问题",
    title: "关于 OpenWarp",
    items: [
      {
        q: "OpenWarp 与 Warp 官方是什么关系?",
        a: "OpenWarp 是基于 Warp 开源代码的社区分支,与 Warp 官方公司无附属关系,遵循上游的 AGPL / MIT 双许可。",
      },
      {
        q: "我的 API Key 会被上传吗?",
        a: "不会。所有自定义提供商凭证仅保存在本地配置文件中,直接由 OpenWarp 与你指定的 Base URL 通信,不经任何中转。",
      },
      {
        q: "支持哪些模型提供商?",
        a: "OpenWarp 内置 genai 多协议适配,原生支持 OpenAI / OpenAI Responses / Anthropic / Gemini / Ollama / DeepSeek 共 6 种协议;OpenAI 兼容端点(Qwen / Groq / Together / OpenRouter / SiliconFlow / LM Studio 等)选 OpenAI 协议并填 Base URL 即可接入。",
      },
      {
        q: "能继续收到 Warp 上游更新吗?",
        a: "会持续合并 Warp 上游主线,在保留体验的同时叠加 BYOP 与多语言增强。",
      },
    ],
  },
  cta: {
    title: "想第一时间体验?",
    desc: "克隆仓库本地构建,或在 GitHub 关注后续发布。",
    button: "前往 GitHub",
    command_label: "克隆仓库",
    command: "git clone https://github.com/zerx-lab/warp",
    copy: "复制",
    copied: "已复制",
    steps: [
      "cargo build --release",
      "./target/release/openwarp",
      "在设置中添加自定义提供商",
    ],
  },
  bento: {
    byop: {
      tag: "模型路由",
      hint: "点击切换提供商",
    },
    privacy: {
      tag: "本地存储",
      bullets: ["不上传云端", "不收集遥测", "凭证零外发"],
    },
    i18n: {
      tag: "可扩展",
      pills: ["简体中文", "English", "日本語", "Español"],
    },
    templates: {
      tag: "minijinja",
      preview: "渲染输出 →",
    },
    warp: {
      tag: "体验保留",
      chips: ["Blocks", "Workflows", "AI 命令", "Keymaps", "主题"],
    },
    opensource: {
      tag: "完全开源",
      license: ["AGPL-3.0", "MIT"],
      links: ["查看源代码", "阅读 LICENSE", "提交 Issue"],
    },
    ssh: {
      tag: "远程连接",
      chips: ["树形分组", "右键 CRUD", "拖拽排序", "Keychain 加密", "凭证自动注入"],
    },
  },
  roadmap: {
    meta_title: "OpenWarp 路线图 — 进行中与计划中的增强",
    meta_description:
      "OpenWarp 在 Warp 上游之上的增强路线图:国际化、客户端分词器多语言、提供商扩展、SSH 管理器。",
    eyebrow: "路线图",
    title: "OpenWarp 路线图",
    subtitle:
      "每一条都对应仓库中已合并或进行中的 commit。绿色=已交付,蓝色=进行中,灰色=计划中。",
    legend: {
      shipped: "已交付",
      in_progress: "进行中",
      planned: "计划中",
    },
    progress_label: "完成度",
    tracks: [
      {
        id: "i18n",
        eyebrow: "01 · 国际化",
        title: "原生多语言界面",
        summary:
          "基于 Fluent (.ftl) 的 i18n 基础设施已就位,英文与简体中文同步迭代,其他语种由社区扩展。",
        progress: 80,
        items: [
          {
            status: "shipped",
            text: "Fluent 基础设施 + ANCHOR 锚点协议(英文与中文同步追加)",
          },
          {
            status: "shipped",
            text: "AI / Features / Teams / Code / MCP Servers / Settings 等设置页端到端翻译",
          },
          {
            status: "shipped",
            text: "workspace 快捷键描述全译(116 key,约 156 处 call site)",
          },
          {
            status: "in_progress",
            text: "剩余 settings_view 子目录与 keybinding-desc 续补",
          },
          {
            status: "planned",
            text: "终端右键菜单、命令面板、Drive 视图等运行时文案补全",
          },
          {
            status: "planned",
            text: "社区扩展:日本語 / Español / 其它语言模板与贡献指南",
          },
        ],
      },
      {
        id: "tokenizer",
        eyebrow: "02 · 客户端分词器",
        title: "面向多语种的输入分类",
        summary:
          "终端输入分类器(input_classifier)原本仅基于英文训练,中文输入容易被误判为 shell 命令。本轨道正在将其扩展到 CJK 与更多书写体系。",
        progress: 35,
        items: [
          {
            status: "shipped",
            text: "CJK 早返回:基本汉字 / 扩展 A / 平假名 / 片假名 / 韩文音节 / 全角标点统一判 AI",
          },
          {
            status: "shipped",
            text: "在 input.rs / agent.rs / universal.rs 等热路径接入 contains_cjk",
          },
          {
            status: "in_progress",
            text: "其他非拉丁脚本:阿拉伯语 / 西里尔字母 / 泰语 / 天城文等的早返回规则",
          },
          {
            status: "planned",
            text: "多脚本混排输入(中英混合)的概率加权,而非硬规则",
          },
          {
            status: "planned",
            text: "替换或补强 natural_language_detection 词典为多语种数据源",
          },
        ],
      },
      {
        id: "providers",
        eyebrow: "03 · 更多提供商",
        title: "BYOP 协议覆盖",
        summary:
          "BYOP 通过 genai 适配层原生支持多种协议,而不仅是 OpenAI Chat Completions。每新增一种原生协议,就减少一层网关与对应的 token 损耗。",
        progress: 60,
        items: [
          {
            status: "shipped",
            text: "OpenAI Chat Completions(GPT-4o / GPT-5 / 任意兼容端点)",
          },
          {
            status: "shipped",
            text: "OpenAI Responses(原生 reasoning / built-in tools)",
          },
          {
            status: "shipped",
            text: "Anthropic 原生(Claude 4.x / 1M context / cache_control)",
          },
          { status: "shipped", text: "Google Gemini 原生协议" },
          { status: "shipped", text: "DeepSeek 原生(推理模型 deepseek-r1)" },
          { status: "shipped", text: "Ollama 本地(零密钥,localhost 直连)" },
          {
            status: "shipped",
            text: "base_url 规范化:host-only 填法自动补 /v1/ 等版本路径",
          },
          {
            status: "in_progress",
            text: "Provider 子页 models.dev 数据源 + 搜索框快速添加",
          },
          { status: "planned", text: "xAI Grok / Mistral / Cohere 原生协议" },
          {
            status: "planned",
            text: "Azure OpenAI / Bedrock / Vertex 等企业网关一键配置模板",
          },
        ],
      },
      {
        id: "active-ai",
        eyebrow: "04 · 主动式 AI",
        title: "主动式 AI 全部走 BYOP",
        summary:
          "Warp 原本的主动式 AI(灰色补全 / Prompt Suggestions / NLD / Relevant Files)默认调用 ${server_root_url}/ai/*,目前已全部切换到 BYOP one-shot,凭证不再经过云端中转。",
        progress: 70,
        items: [
          {
            status: "shipped",
            text: "Agent 主对话流走 BYOP(genai 6 协议显式路由)",
          },
          {
            status: "shipped",
            text: "Next Command 灰色补全 + zero-state 建议切到 BYOP one-shot",
          },
          {
            status: "shipped",
            text: "Prompt Suggestions / NLD predict / Relevant Files 全量切到 BYOP",
          },
          {
            status: "shipped",
            text: "新增 active_ai_model / next_command_model 独立模型字段",
          },
          {
            status: "shipped",
            text: "DeepSeek reasoning_content 多轮回传(genai DeepSeek adapter)",
          },
          {
            status: "shipped",
            text: "BYOP LRC tag-in 多轮上下文持续注入 + sanitize 双向补 placeholder",
          },
          {
            status: "in_progress",
            text: "Code Review(commit message / PR title / PR description)接入 BYOP",
          },
          {
            status: "planned",
            text: "passive suggestions(Workflow / Rule chips)BYOP 化",
          },
        ],
      },
      {
        id: "decouple",
        eyebrow: "05 · 解耦云端",
        title: "切断默认云端依赖",
        summary:
          "OpenWarp 是纯本地分支:云账号刷新、远端用户持久化、Plan 同步、passive suggestions HTTP 等连回 Warp Inc 的链路均已就地禁用,凭证与请求只发往你配置的服务商。",
        progress: 75,
        items: [
          { status: "shipped", text: "移除 Cloud Agent / Computer Use 入口" },
          {
            status: "shipped",
            text: "auth_manager refresh_user / persist 整体 no-op,不再向 app.warp.dev 写入用户态",
          },
          {
            status: "shipped",
            text: "移除 Plan 自动同步 Warp Drive 开关与调用",
          },
          {
            status: "shipped",
            text: "passive suggestions 云端 HTTP 链路短路 + 静默 modal warn",
          },
          {
            status: "shipped",
            text: "Profile Editor 残留云端开关清理(autosync / web search)",
          },
          {
            status: "in_progress",
            text: "i18n 文案去除「云端 Agent / Oz」字样",
          },
          {
            status: "planned",
            text: "彻底审计仍然命中 ${server_root_url} 的所有路径",
          },
        ],
      },
      {
        id: "polish",
        eyebrow: "06 · 体验 & 稳定性",
        title: "体验打磨与稳定性",
        summary:
          "围绕 BYOP 多协议补齐细节:多轮 tool_use 配对、Take over Agent 切回、长命令 alt-screen 防卡死、命令面板中英双向搜索,以及 OpenWarp 打包与 Release 工作流。",
        progress: 65,
        items: [
          {
            status: "shipped",
            text: "BYOP 多轮 tool_use 双向 sanitize:孤儿 tool_response 不再触发 Anthropic 400 + 重试 flex panic",
          },
          {
            status: "shipped",
            text: "TUI / 长命令 Take over Agent → resume 链路修复(SetInputModeAgent alt-screen 死锁)",
          },
          {
            status: "shipped",
            text: "footer 工具提示 context_window_usage 实时同步(BYOP usage_metadata 透传)",
          },
          {
            status: "shipped",
            text: "命令面板 Fuzzy 搜索 + binding.name 中英双向匹配",
          },
          {
            status: "shipped",
            text: "63 个 Toggle 设置命令前后缀全译(Fluent {$suffix} 占位)",
          },
          {
            status: "shipped",
            text: "Windows 打包名 WarpOss → OpenWarp 对齐",
          },
          {
            status: "shipped",
            text: "macOS Release timeout 90 → 150 分钟",
          },
          {
            status: "planned",
            text: "Linux Release 工作流自动化",
          },
        ],
      },
      {
        id: "ssh",
        eyebrow: "07 · SSH 管理器",
        title: "内置 SSH 管理器",
        summary:
          "侧边栏新增 SSH 管理器面板:树形分组、右键 CRUD、拖拽排序,密钥与密码加密存入系统 keychain;单击即连,首次登录自动注入密码或 passphrase。",
        progress: 70,
        items: [
          {
            status: "shipped",
            text: "SQLite ssh_nodes 表 + 服务器 / 文件夹双类型 + parent_id 树结构",
          },
          {
            status: "shipped",
            text: "本地 keychain 加密保存 password / key passphrase,凭证零外发",
          },
          {
            status: "shipped",
            text: "侧边栏树视图:文件夹折叠 / 展开 + 状态持久化 + 智能 toggle-all",
          },
          {
            status: "shipped",
            text: "右键菜单 CRUD(新建 / 编辑 / 删除 / 连接)+ 文件夹内联重命名",
          },
          {
            status: "shipped",
            text: "拖拽移动节点:server / folder 任意嵌套 + 环检测 reject",
          },
          {
            status: "shipped",
            text: "中央 Pane 编辑器(Drive 风格):name / host / port / user + 密码 / 私钥 pill toggle + Save",
          },
          {
            status: "shipped",
            text: "Connect 一键开新 terminal pane,自动写入 ssh 命令",
          },
          {
            status: "shipped",
            text: "SecretInjector:订阅 PTY 输出,15s 滑窗匹配 password / passphrase prompt 并注入",
          },
          {
            status: "in_progress",
            text: "导入 ~/.ssh/config 已有主机批量入库",
          },
          {
            status: "planned",
            text: "SFTP / 端口转发 / Jump host 链路配置",
          },
          {
            status: "planned",
            text: "服务器健康探测 + 颜色状态指示",
          },
        ],
      },
    ],
    footnote_title: "路线图怎么读",
    footnote_body:
      "本路线图按已合并的提交维护,而非愿望清单。每个 ✓ 都对应代码库中的具体文件与函数,进行中条目对应已开 issue 或已起草 PR。欢迎到 GitHub 提 issue 或 PR 参与共建。",
    cta_repo: "查看仓库",
    cta_issues: "提交 issue",
  },
  footer: {
    project: "项目",
    community: "社区",
    legal: "法律",
    docs: "文档",
    changelog: "更新日志",
    roadmap: "路线图",
    discussions: "讨论",
    issues: "问题反馈",
    license: "许可协议",
    privacy: "隐私",
    rights: "基于 Warp 的社区分支,与 Warp 官方无关",
  },
};

export type Dict = typeof zhCN;
