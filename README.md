# USML — Usecase Markup Language

OpenAPI と DBML の間のデータフローを声明的に定義する言語です。API レスポンスフィールドがどのDBテーブル・カラムから来るかを明示的に記述し、JOIN・集約・変換のロジックを一つの YAML ファイルで管理できます。

## Features

- **OpenAPI・DBML 参照インポート** — 外部スキーマファイルを直接参照して検証
- **レスポンスマッピング** — フィールド→ソース対応の明示的定義
- **JOIN・JOIN Chain** — 単一結合と多段結合の両方に対応
- **Aggregate** — COUNT/SUM/AVG/MIN/MAX と GROUP BY
- **Transforms** — COALESCE/CONCAT/CASE/MASK/CONDITIONAL_SOURCE
- **12規則バリデーション** — パス・メソッド・カラム・パラメータの存在確認まで
- **インタラクティブ可視化** — タブ切り替え可能な2つのビュー
  - **ビジュアルビュー**: ホバーハイライト付きデータフロー図（3カラムレイアウト）
  - **テーブルビュー**: 階層構造を持つResponse Mappingテーブル
  - **API情報表示**: OpenAPIから抽出したメソッド・パス・ステータスコードを自動表示
- **VS Code拡張** — 自動バリデーション・データフロー図プレビュー

## Installation

```sh
# Rust を事前にインストールが必要
git clone https://github.com/Nenene01/usml.git
cd usml
cargo build --release
# バイナリ: target/release/usml
```

## Usage

### バリデーション

```sh
usml validate examples/users-list.usml.yaml
```

JSON 形式で出力（CI・拡張連携用）:

```sh
usml validate --json examples/users-list.usml.yaml
```

### AST 確認

```sh
usml parse examples/users-list.usml.yaml
```

### データフロー図生成

```sh
# デフォルト: ./output/<usecase-name>.html に出力
usml visualize examples/users-list.usml.yaml
# → 出力: output/ユーザー一覧取得.html

# カスタムパスに出力 (-o または --output)
usml visualize examples/users-list.usml.yaml -o custom.html
usml visualize examples/users-list.usml.yaml --output flow.html
```

**出力先の優先順位:**
1. `-o/--output` オプション（最優先）
2. USMLファイル内の `usecase.output` パラメータ
3. デフォルト: `./output/<usecase.name>.html`

**生成されるHTML の機能:**
- **タブ切り替え**: テーブルビュー ⇄ ビジュアルビュー
- **OpenAPI情報**: ヘッダーにHTTPメソッド・APIパス・ステータスコードを表示
- **ビジュアルビュー**:
  - 3カラムレイアウト（Response Fields / Joins & Transforms / Tables）
  - Joins & Transformsカードに種類バッジ（Simple / JOIN / JOIN Chain / Aggregate）を表示
  - ホバーで関連要素をハイライト（Response Fields → Joins & Transforms → Tables の対応関係）
  - ネストされたフィールドは階層構造で色分け表示
  - エイリアステーブルは「実テーブル名 (as エイリアス)」形式で表示
- **テーブルビュー**:
  - Response Mapping: フィールド・ソース・JOIN・変換を階層構造で表示
  - Tables Summary: 使用されるテーブルとカラムの一覧（エイリアス表示対応）
  - Filters: フィルタパラメータ・種類・詳細情報の一覧
  - Transforms: 変換ロジックの詳細情報

## USML 構文

```yaml
version: "0.1"

import:
  openapi: ./api.yaml#paths["/users"].get.responses["200"]
  dbml:
    - ./schema.dbml#tables["users"]
    - ./schema.dbml#tables["profiles"]

usecase:
  name: ユーザー一覧取得
  summary: ページネーション付きのユーザー一覧を返す
  output: users-list.html  # オプション: 可視化HTMLのファイル名

  response_mapping:
    - field: id
      source: users.id
    - field: avatar_url
      source: profiles.avatar_url
      join:
        table: profiles
        on: users.id = profiles.user_id
    - field: display_name
      source: profiles.display_name

  filters:
    - param: status
      maps_to: WHERE
      condition: users.status = :status
    - param: page
      maps_to: PAGINATION
      strategy: offset
      page_size: 20

  transforms:
    - target: display_name
      type: COALESCE
      sources:
        - profiles.display_name
        - users.name
```

## VS Code 拡張

`extensions/vscode/` ディレクトリに拡張のソースがあります。

- `.usml.yaml` ファイルの保存時に自動バリデーション
- `USML: データフロー図を開く` コマンドでWebviewプレビュー
- `usml.binaryPath` 設定でバイナリパス指定可能

## Project Structure

```
usml/
├── cli/src/main.rs          # CLI エントリポイント (validate/parse/visualize)
├── core/src/
│   ├── ast.rs               # AST 型定義
│   ├── parser.rs            # YAML → AST パーサー
│   ├── validator.rs         # 12規則バリデーション + リゾルバー統合
│   ├── visualizer.rs        # インタラクティブHTMLデータフロー図生成
│   └── resolver/
│       ├── dbml.rs          # DBML ファイル解析
│       └── openapi.rs       # OpenAPI ファイル解析
├── extensions/vscode/       # VS Code 拡張
├── examples/                # サンプル USML ファイル
├── output/                  # 生成されたHTMLファイル（デフォルト出力先）
└── docs/spec/               # USML 仕様ドキュメント
```

## License

MIT
