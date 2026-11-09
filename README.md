# EzyMcBeSrv

[Chorus](https://github.com/bedrock-crustaceans/chorus) のサーバーを取得・ビルド・操作する Rust 製 TUI です。

## 必要なもの

- Rust / Cargo（Chorus のビルドにも使用）
- Git（Chorus の取得に使用）
- ビルドに必要なディスク容量と時間

## 起動

```sh
cargo run --release
```

既定の保存先は Linux では `$XDG_DATA_HOME/ezymcbesrv` または `~/.local/share/ezymcbesrv`、Windows では `%APPDATA%/ezymcbesrv` です。別の場所を使う場合は `cargo run --release -- --data-dir /path/to/data` を指定します。

| キー | 操作 |
| --- | --- |
| `n` | 新規サーバー作成（名前入力→Git clone→リリースビルド） |
| `↑` / `↓` | サーバー選択 |
| `b` | 選択したサーバーの再ビルド（稼働中は不可） |
| `s` | 起動 |
| `x` | `stop` を送信して正常終了 |
| `c` | Chorus のコンソールへコマンドを送信 |
| `l` | ビルドログとサーバーログを切り替え |
| `q` | 終了（サーバー稼働中・ビルド中は不可） |

各サーバーは `servers/<名前>/source`（ソースコード）と `servers/<名前>/data`（ワールドや設定）に分離します。ビルドの出力は `build.log`、実行時の出力は `server.log` に保存します。サーバーは TUI の子プロセスとして動作するため、TUI を終了する前に `x` で停止してください。現在の版ではプロセスを外部から起動・再接続する機能、削除機能、自動更新機能はありません。

Chorus 本体はこのアプリに同梱されません。取得元は `https://github.com/bedrock-crustaceans/chorus` です。Chorus のライセンスは Apache-2.0 です。
