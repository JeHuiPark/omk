# EPIC-DATA-01: 지식 데이터 모델 및 출처 관리 (Knowledge Data Model & Provenance)

## 1. Introduction
### 1.1 Purpose
이 문서는 `omk` 시스템에 저장되는 지식 데이터의 논리적/물리적 모델을 정의하고, 특히 지식의 출처(Provenance)와 신뢰도(Reliability)를 비즈니스 환경에 맞게 세분화하여 관리하는 기준을 명세합니다.

### 1.2 Scope
- 마크다운 기반의 물리적 저장 구조 (YAML Frontmatter)
- 검색 및 확장을 위한 SQLite 기반 인덱스 모델
- 지식 출처(Source)의 세분화 및 갱신 전략(Sync Strategy)

## 2. Source Provenance & Sync Strategy (출처 및 갱신 모델)

실제 비즈니스 환경에서는 다양한 출처에서 데이터가 유입되므로, `EXTERNAL_SYNC`를 다차원적으로 세분화하여 관리합니다.

### 2.1 Source Class (대분류)
지식이 생성된 주체와 근본적인 권위를 정의합니다.
1. `USER_EXPLICIT`: 사용자가 직접 입력하거나 명시적으로 저장을 지시한 지식. (최고 권위, AI 임의 수정 불가)
2. `AGENT_INFERRED`: AI 에이전트가 자율적으로 추론하여 저장한 지식. (가변적, 덮어쓰기 허용)
3. `EXTERNAL_SYNC`: 외부 비즈니스 시스템(위키, 메신저 등)에서 동기화된 지식.

### 2.2 Source Type & Sync Strategy (외부 동기화 세분화)
`EXTERNAL_SYNC` 클래스의 경우, 데이터 특성에 따라 갱신 전략(Sync Strategy)을 다르게 적용합니다.

| Source Type | 특성 (Data Nature) | 주요 Provider | 갱신 전략 (Sync Strategy) |
| :--- | :--- | :--- | :--- |
| **`WIKI`** | **문서 중심 (Document)**<br>구조화된 공식 정보 | Notion, Confluence | **`UPSERT_FULL` (전체 덮어쓰기)**<br>고유 ID 기반 최신 버전 전체 갱신 |
| **`MESSENGER`** | **스트림 중심 (Stream)**<br>시간순 배열, 스레드 중요 | Slack, MS Teams | **`APPEND_ONLY` (추가/요약 병합)**<br>원본 타임라인 훼손 없이 추가 및 요약 |
| **`ISSUE_TRACKER`**| **상태 중심 (Stateful)**<br>진행 상태(Open/Closed) 핵심 | Jira, GitHub Issues | **`STATE_PATCH` (부분 업데이트)**<br>상태는 덮어쓰고, 댓글은 Append 하는 하이브리드 방식 |
| **`CODE_REPO`** | **버전 중심 (Versioned)**<br>파일 경로와 커밋 해시 기준 | GitHub, GitLab | **`LINK_REFERENCE` (참조 위주)**<br>코드 스니펫과 커밋 링크 참조 저장 |

## 3. Data Model

### 3.1 물리적 데이터 모델 (Markdown + YAML Frontmatter)
마크다운 파일은 단일 진실 공급원(SSOT)이며, 아래와 같은 구조화된 메타데이터를 포함합니다.

```yaml
---
id: "01HGWZ..."               
title: "결제 시스템 아키텍처 개편안"

# 1. 출처 및 권위 (Provenance & Authority)
source:
  class: "EXTERNAL_SYNC"
  type: "WIKI"
  provider: "confluence"
  uri: "https://confluence.company.com/pages/12345"
  external_id: "PAGE-12345"

# 2. 동기화 및 생명주기 (Sync & Lifecycle)
sync:
  strategy: "UPSERT_FULL"
  last_synced_at: "2026-07-17T16:15:00Z"
  version_hash: "a1b2c3d4..."

# 3. 평가 지표 (Metrics)
metrics:
  authority: 0.9               # 권위 (출처의 공식성)

# 4. 비즈니스 메타데이터 (동적 확장 필드)
tags: ["architecture", "payment"]
author: "alice@company.com"
---
본문...
```

### 3.2 시스템 DB 인덱스 모델 (SQLite)
빠른 필터링과 스키마 변경 없는 유연한 확장을 위해 **JSON 컬럼**을 활용합니다.

- **`knowledge_index` 테이블:**
  - `id` (TEXT, PK)
  - `title` (TEXT)
  - `source_class` (TEXT)
  - `source_type` (TEXT)
  - `authority` (REAL)
  - `created_at` (INTEGER)
  - `metadata` (JSON) - Frontmatter의 동적 필드(tags, uri, external_id 등) 저장
- **`knowledge_fts` 테이블 (SQLite FTS5):**
  - 마크다운 본문을 토큰화하여 전문 검색 지원.
