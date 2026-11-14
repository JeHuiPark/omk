# EPIC-SEARCH-01: 지식 검색 및 인덱싱 (Knowledge Search and Indexing)

## 1. Introduction
### 1.1 Purpose
본 스펙은 초기 빠른 출시를 목표로 무거운 벡터 임베딩 모델 대신, 가볍고 빠른 로컬 Full-Text Search(FTS) 및 메타데이터 검색 기능을 구현하는 것을 목적으로 합니다.

### 1.2 Scope
- SQLite FTS5 또는 Tantivy 기반의 로컬 텍스트 인덱싱 엔진.
- 파일의 본문 텍스트 및 YAML Frontmatter(태그, 엔티티)에 대한 복합 검색.

## 2. Overall Description
### 2.1 Product Perspective
LLM 기반의 AI 에이전트가 뛰어난 추론 능력을 바탕으로 모호한 질의를 구조화된 검색어(키워드 모음)로 변환하고, `omk CLI`는 이 검색어를 받아 초고속 텍스트 매칭 결과를 반환하는 '역할 분담' 구조를 갖습니다.

## 3. System Features

### 3.1 REQ-SEARCH-001: 전문 검색 (Full-Text Search)
- **3.1.1 Description and Priority:** 높은 우선순위. 마크다운 본문 및 제목 내의 특정 키워드나 구문을 빠르고 정확하게 검색합니다.
- **3.1.2 Stimulus/Response Sequences:**
  - *Stimulus:* 에이전트가 `search_knowledge` 도구 호출 (데이터: 검색 쿼리 문자열).
  - *Response:* 매칭된 파일 목록 및 매칭된 구문의 스니펫(Snippet) 반환.
- **3.1.3 Functional Requirements:**
  - 쿼리 토큰 단위의 논리 연산(AND, OR) 매칭을 지원해야 한다.
  - 결과에는 `file_path`, `title`, `snippet` 필드가 포함되어야 한다.

### 3.2 REQ-SEARCH-002: 메타데이터 및 태그 필터링
- **3.2.1 Description and Priority:** 중간 우선순위. 지식의 분류(Category), 태그(Tags) 등 메타데이터에 기반한 정밀 필터링을 지원합니다.
- **3.2.2 Stimulus/Response Sequences:**
  - *Stimulus:* 에이전트가 필터 조건이 포함된 검색 요청을 전송.
  - *Response:* 필터 조건에 부합하는 결과만 반환.
- **3.2.3 Functional Requirements:**
  - 마크다운 파일 생성 시 갱신되는 로컬 DB (SQLite 등)를 통해 태그 일치 여부를 판별해야 한다.
