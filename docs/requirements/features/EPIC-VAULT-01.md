# EPIC-VAULT-01: Markdown 기반 영구 지식 저장소 (Markdown-based Persistent Knowledge Storage)

## 1. Introduction
### 1.1 Purpose
본 스펙은 AI 에이전트가 단기 기억(대화 컨텍스트)의 한계를 극복하고 사용자의 지식을 영구적으로 저장할 수 있는 로컬 지식 금고(Local Vault)의 구조와 동작을 명세합니다.

### 1.2 Scope
- 로컬 파일 시스템 내의 특정 디렉터리를 기준(Workspace)으로 마크다운(.md) 파일들을 관리합니다.
- `omk CLI`가 파일의 읽기, 쓰기, 수정(Upsert)을 엄격하게 통제합니다.

## 2. Overall Description
### 2.1 Product Perspective
단일 정적 바이너리로 구동되는 `omk CLI`는 파일 I/O를 직접 통제하여 데이터 무결성을 보장하며, AI 에이전트는 제공되는 인터페이스를 통해서만 파일에 접근할 수 있습니다.

### 2.2 User Classes and Characteristics
- **AI Agent:** 로컬 금고에 지식을 구조화하여 저장하고 필요 시 읽어오는 클라이언트.
- **Human User:** 백업, 동기화, 또는 직접 마크다운 편집을 통해 금고 내용을 확인할 수 있는 주체.

## 3. System Features

### 3.1 REQ-VAULT-001: 지식 구조화 및 저장 (Upsert Knowledge)
- **3.1.1 Description and Priority:** 높은 우선순위. 에이전트가 새로운 지식(Entity, Summary 등)을 마크다운 포맷으로 금고에 저장합니다. 기존 지식과 충돌 시 병합(Merge) 또는 덮어쓰기(Overwrite)를 수행합니다.
- **3.1.2 Stimulus/Response Sequences:** 
  - *Stimulus:* 에이전트가 `upsert_knowledge` 도구 호출 (데이터: 파일명, 내용, 메타데이터).
  - *Response:* `omk CLI`가 파일을 생성/수정 후 성공 응답 반환.
- **3.1.3 Functional Requirements:**
  - 파일은 반드시 UTF-8 인코딩된 `.md` 확장자로 저장되어야 한다.
  - 마크다운 파일의 상단에는 파일 메타데이터 관리를 위한 YAML Frontmatter(예: 태그, 생성일, 연관 엔티티)가 포함되어야 한다.

### 3.2 REQ-VAULT-002: 지식 단건 조회 (Read Knowledge)
- **3.2.1 Description and Priority:** 높은 우선순위. 특정 지식(파일)을 명확한 식별자(파일 경로 또는 ID)를 통해 즉각적으로 읽어옵니다.
- **3.2.2 Stimulus/Response Sequences:**
  - *Stimulus:* 에이전트가 `read_knowledge` 도구 호출.
  - *Response:* 파일의 텍스트 내용을 반환. (파일이 존재하지 않으면 에러 반환)
- **3.2.3 Functional Requirements:**
  - 지정된 파일의 텍스트 컨텐츠를 훼손 없이 반환해야 한다.
  - 보안을 위해 `omk`가 관리하는 Vault 디렉터리 외부의 파일 접근(Path Traversal)은 엄격히 차단되어야 한다.
