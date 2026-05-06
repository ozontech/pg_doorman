/**
 * DTO mirrors. Phase 5 only exposes the types phase 5 actually uses; phase 6
 * adds the rest as pages need them. Source of truth is `src/web/routes/dto.rs`
 * — keep these manual until divergence becomes painful.
 */
export interface VersionDto {
  version: string;
  git_commit: string;
  build_date: string;
  ts: number;
}
