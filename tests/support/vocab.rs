//! Curated word lists for the synthetic chat corpus. Lists are whitespace-separated so they stay
//! compact; `corpus.rs` expands them (places x suffixes, surnames x given names, domains x paths)
//! into a vocabulary of several thousand entries and samples each category with a Zipf law.
//!
//! None of these lists may contain a needle (see `NEEDLE_CHARS`/`NEEDLE_WORDS` in `corpus.rs`);
//! the quality test verifies that planted counts are exact.

pub const COMMON_ZH: &str = "今天 明天 昨天 現在 剛剛 等一下 馬上 最近 以前 之後 晚上 早上 中午 下午 週末 假日 \
禮拜一 禮拜五 朋友 同事 老闆 老師 同學 家人 爸爸 媽媽 小孩 女朋友 男朋友 工作 上班 下班 加班 開會 會議 報告 \
專案 進度 問題 答案 方法 辦法 時間 地方 事情 東西 東西 感覺 想法 意思 理由 原因 結果 可以 不行 沒有 知道 \
不知道 應該 可能 一定 真的 其實 因為 所以 但是 如果 還是 或者 已經 還沒 需要 想要 喜歡 討厭 覺得 希望 \
決定 準備 開始 結束 完成 處理 解決 確認 通知 提醒 回覆 回來 出門 回家 吃飯 午餐 晚餐 早餐 宵夜 便當 \
飲料 手搖 珍珠奶茶 咖啡 咖啡廳 拿鐵 美式 蛋糕 甜點 火鍋 燒烤 拉麵 牛肉麵 滷肉飯 雞排 蚵仔煎 小籠包 \
豆漿 蛋餅 麵包 水果 西瓜 芒果 草莓 蘋果 香蕉 好吃 好喝 好看 好玩 好用 好笑 好累 好忙 好冷 好熱 \
很貴 很便宜 很快 很慢 很大 很小 很多 很少 超級 非常 有點 稍微 完全 一直 常常 偶爾 通常 差不多 大概 \
大家 我們 你們 他們 自己 別人 有人 沒人 什麼 怎麼 為什麼 哪裡 誰的 多少 幾點 幾個 一些 全部 部分 \
謝謝 不客氣 對不起 沒關係 拜託 麻煩 辛苦了 加油 恭喜 生日快樂 晚安 早安 你好 再見 掰掰 收到 了解 \
好的 沒問題 OK啦 真假 對啊 是喔 好喔 當然 不會吧 太誇張 受不了 傻眼 無言 厲害 讚 棒 超棒 太強了 \
天氣 下雨 颱風 太陽 很熱 很冷 空調 冷氣 暖氣 溫度 雨傘 外套 衣服 鞋子 包包 眼鏡 手錶 錢包 鑰匙 \
手機 電腦 筆電 平板 耳機 充電 電池 螢幕 鍵盤 滑鼠 相機 照片 影片 音樂 電影 電視 遊戲 小說 漫畫 動畫 \
演唱會 展覽 旅行 旅遊 出國 機票 飯店 民宿 行李 護照 簽證 景點 逛街 購物 網購 折扣 優惠 免運 退貨 \
訂單 付款 發票 信用卡 現金 轉帳 匯款 帳單 房租 水電 薪水 獎金 預算 保險 貸款 股票 基金 投資 理財 \
考試 作業 成績 畢業 研究所 大學 高中 國中 小學 補習 教室 圖書館 宿舍 社團 活動 比賽 運動 跑步 健身 \
游泳 騎車 騎腳踏車 開車 搭車 公車 火車 高鐵 計程車 機車 停車 塞車 紅燈 綠燈 路口 加油站 \
醫生 醫院 看診 感冒 發燒 咳嗽 頭痛 肚子痛 吃藥 休息 睡覺 失眠 熬夜 起床 刷牙 洗澡 打掃 洗衣服 \
貓咪 狗狗 寵物 散步 公園 河濱 海邊 爬山 露營 烤肉 聚餐 約會 結婚 婚禮 紅包 過年 中秋 端午 聖誕節 \
新聞 政治 選舉 經濟 社會 文化 歷史 科學 藝術 設計 攝影 寫作 閱讀 學習 練習 分享 討論 建議 意見 \
消息 訊息 群組 頻道 貼圖 語音 視訊 電話 簡訊 郵件 附件 連結 網址 網站 網頁 帳號 密碼 登入 登出 註冊";

pub const FUNCTION: &str = "的 了 在 是 我 你 他 她 也 就 都 還 有 不 嗎 吧 啊 呢 喔 欸 啦 哈 嘿 唉 哇 蛤 \
這 那 個 和 與 跟 而 或 及 很 會 要 去 來 到 說 看 想 做 好 對 沒 但 又 被 把 讓 給 比 才 只 再";

pub const PLACES: &str = "台北 新北 桃園 新竹 苗栗 台中 彰化 南投 雲林 嘉義 台南 高雄 屏東 宜蘭 花蓮 台東 基隆 澎湖 金門 \
馬祖 板橋 中和 永和 新店 淡水 三重 蘆洲 汐止 內湖 南港 信義 大安 中山 松山 士林 北投 萬華 文山 木柵 \
天母 東區 西門町 公館 師大 士林 九份 平溪 烏來 陽明山 貓空 象山 華山 松菸 大稻埕 迪化街 龍山寺 \
中壢 竹北 竹東 湖口 豐原 大甲 逢甲 草屯 鹿港 斗六 北港 朴子 新營 安平 赤崁 鳳山 左營 三民 旗津 \
美濃 墾丁 恆春 羅東 礁溪 蘇澳 太魯閣 七星潭 池上 鹿野 綠島 蘭嶼 日月潭 阿里山 清境 合歡山 梨山";

pub const PLACE_SUFFIX: &str =
    "車站 夜市 咖啡廳 捷運站 老街 附近 市區 路口 公園 商圈 分店 好吃 景點 天氣 交流道 轉運站";

pub const TECH_ZH: &str = "伺服器 資料庫 硬碟 記憶體 處理器 顯示卡 主機板 網路 路由器 防火牆 頻寬 延遲 效能 優化 快取 索引 \
查詢 事務 備份 還原 部署 上線 下線 重啟 重新啟動 當機 崩潰 漏洞 修補 更新 升級 降級 版本 發佈 分支 合併 \
提交 程式碼 程式 演算法 函式 變數 類別 介面 模組 套件 依賴 編譯 執行 除錯 測試 單元測試 整合測試 覆蓋率 \
日誌 監控 告警 容器 映像檔 叢集 節點 負載平衡 微服務 後端 前端 全端 工程師 開發者 資安 加密 憑證 授權 \
驗證 令牌 金鑰 雲端 虛擬機 網域 憑證 代理 爬蟲 搜尋 全文檢索 斷詞 分詞 中文 繁體 簡體 編碼 亂碼 時區 \
資料 數據 檔案 目錄 權限 排程 佇列 並行 非同步 同步 執行緒 記憶體洩漏 垃圾回收 型別 泛型 所有權 生命週期 \
設定檔 環境變數 命令列 終端機 腳本 自動化 持續整合 持續部署 工單 需求 規格 文件 範例 教學 開源 授權條款";

pub const TECH_EN: &str = "gitlab github docker kubernetes k8s nginx redis postgres postgresql mysql sqlite mongodb kafka rabbitmq \
linux ubuntu debian archlinux macos windows android ios chrome chromium firefox safari vscode vim neovim emacs \
rust cargo tokio axum sqlx serde python pip django flask fastapi javascript typescript nodejs npm yarn react vue \
svelte webpack vite golang java kotlin swift ruby rails php laravel docker-compose terraform ansible jenkins \
runner pipeline commit branch merge rebase pull request issue release deploy staging production hotfix rollback \
api rest graphql grpc json yaml toml xml http https tcp udp dns ssl tls ssh vpn cdn aws gcp azure cloudflare \
cpu gpu ram ssd nvme usb wifi bluetooth bug feature refactor lint test mock cache latency throughput timeout \
regex jieba tokenizer bigram fts5 index query schema migration backup restore telegram bot webhook oauth jwt \
llm gpt prompt embedding vector profiler flamegraph valgrind gdb lldb wasm webassembly";

pub const EN_WORDS: &str = "the and for with this that have from they will would there their what about which when make like \
time just know take people into year your good some could them see other than then now look only come its over \
think also back after use two how our work first well way even new want because any these give day most us \
meeting deadline sorry thanks please okay yes no maybe later today tomorrow weekend lunch dinner coffee tea \
home office team project update status report review plan design budget price order pay free sale discount \
happy birthday congrats welcome morning night weather movie music game phone laptop screen battery charger \
lol omg btw thx pls asap fyi imo idk brb gg wtf haha nice cool great awesome cute ok";

pub const SLANG: &str = "笑死 哈哈哈 哈哈哈哈 XD 87 母湯 北鼻 好扯 超扯 崩潰 爆笑 傻眼貓咪 破防 躺平 內卷 社畜 肝 爆肝 \
歐趴 歐趴糖 揪 揪團 揪咪 沒碗糕 尬聊 尷尬 母雞 欸欸 對啊對啊 是在哈囉 超好笑 太可以了 讚讚 蝦 蝦米 \
啥 屁啦 靠北 靠腰 北爛 雷 雷包 暈 ㄏㄏ ㄎㄎ 87分 888 666 +1 -1 ++ orz OTZ QQ 嗚嗚 嗚嗚嗚 ㄇㄉ 森77 \
GG 好家在 有夠 超 贏麻了 穩 穩了 笑到 笑翻 哭哭 傻住 暈爆 煩死了 累爆 餓死了 想睡 好想睡 好想吃";

pub const EMOJI: &str = "😂 😭 🤣 😅 😊 😍 🥺 😎 🤔 😴 😡 🙄 👍 👎 👏 🙏 💪 🎉 🔥 ✨ ❤️ 💔 😆 🤗 😱 🥲 🤡 🐶 🐱 ☕ 🍜 🍺 🍰 🚀 💯 ✅ ❌ ⚠️ 📌";

pub const SURNAMES: &str = "陳 林 黃 張 李 王 吳 劉 蔡 楊 許 鄭 謝 郭 洪 曾 邱 廖 賴 徐 周 葉 蘇 莊 呂 江 何 蕭 羅 高 潘 簡 朱 鍾 彭 游 詹 胡 施 沈";

pub const GIVEN_CHARS: &str = "怡 君 家 豪 志 明 雅 婷 俊 傑 宗 翰 佩 珊 欣 穎 宜 庭 柏 宇 冠 廷 子 軒 詩 涵 品 妤 睿 哲 承 恩 佳 蓉 淑 芬 美 玲 建 宏 文 彥 偉 良 正 國 信 昌 秀 英 慧 敏 靜 婉 如 筱 禹 學 凱 昱 瑋 銘 育 亭 湘 心 筠 芷 安";

pub const NICK_PREFIX: &str = "阿 小 老 大";
pub const NICK_CHARS: &str =
    "明 華 傑 豪 偉 志 強 芬 美 玲 凱 翔 宏 龍 虎 貓 狗 熊 魚 豬 牛 馬 哲 宇 軒 恩 安 琪 琳 欣";

pub const EN_NAMES: &str = "Kevin Jason Amy Emily David Michael Sarah Jessica Tom Jerry Eric Peter Vincent Ivy Cindy Allen Alex Ben Chris Dan Eddie Frank Gary Henry Jack Leo Mike Nick Oscar Paul Ray Sam Tony Wendy Yuki";

pub const DOMAINS: &str = "github.com gitlab.com example.com youtube.com youtu.be twitter.com facebook.com instagram.com ptt.cc dcard.tw \
medium.com stackoverflow.com wikipedia.org google.com news.yahoo.com ithome.com.tw kkday.com line.me t.me docs.rs crates.io npmjs.com";

pub const URL_WORDS: &str =
    "watch blob issues pull wiki docs article post status tree commit releases search topics user";

pub const MARKS_ZH: &str = "。 ！ ？ ， ； … ～ 、";
pub const MARKS_ASCII: &str = ". ! ? , ;";

/// Traditional -> Simplified for the "some messages are Simplified" part of the corpus.
pub const SIMPLIFIED: &[(char, char)] = &[
    ('體', '体'),
    ('這', '这'),
    ('們', '们'),
    ('說', '说'),
    ('嗎', '吗'),
    ('會', '会'),
    ('個', '个'),
    ('時', '时'),
    ('開', '开'),
    ('對', '对'),
    ('為', '为'),
    ('還', '还'),
    ('後', '后'),
    ('學', '学'),
    ('來', '来'),
    ('裡', '里'),
    ('點', '点'),
    ('電', '电'),
    ('腦', '脑'),
    ('資', '资'),
    ('訊', '讯'),
    ('網', '网'),
    ('軟', '软'),
    ('碼', '码'),
    ('設', '设'),
    ('備', '备'),
    ('發', '发'),
    ('現', '现'),
    ('問', '问'),
    ('題', '题'),
    ('見', '见'),
    ('長', '长'),
    ('門', '门'),
    ('間', '间'),
    ('車', '车'),
    ('讓', '让'),
    ('與', '与'),
    ('無', '无'),
    ('過', '过'),
    ('讀', '读'),
    ('寫', '写'),
    ('買', '买'),
    ('賣', '卖'),
    ('錢', '钱'),
    ('記', '记'),
    ('認', '认'),
    ('語', '语'),
    ('請', '请'),
    ('謝', '谢'),
    ('應', '应'),
    ('該', '该'),
    ('實', '实'),
    ('驗', '验'),
    ('數', '数'),
    ('據', '据'),
    ('庫', '库'),
    ('動', '动'),
    ('運', '运'),
    ('轉', '转'),
    ('連', '连'),
    ('線', '线'),
    ('紅', '红'),
    ('藍', '蓝'),
    ('綠', '绿'),
    ('飛', '飞'),
    ('機', '机'),
    ('場', '场'),
    ('灣', '湾'),
    ('務', '务'),
    ('器', '器'),
    ('雞', '鸡'),
    ('麵', '面'),
    ('飯', '饭'),
    ('館', '馆'),
    ('廳', '厅'),
    ('經', '经'),
    ('濟', '济'),
];
