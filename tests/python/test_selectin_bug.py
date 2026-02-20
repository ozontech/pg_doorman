"""
Reproduces TypeError in SQLAlchemy asyncpg dialect when using pg_doorman
in transaction pool mode with selectin-loaded relationships.

Error: TypeError: expected string or bytes-like object, got 'NoneType'
at sqlalchemy/dialects/postgresql/asyncpg.py _prepare_and_execute() -> re.match(pattern, status)

The `status` (CommandComplete tag like "SELECT 1") becomes None when pg_doorman's
prepared statement caching interferes with asyncpg's extended query protocol flow.

Usage:
    # Through pg_doorman (expect error):
    python test_selectin_bug.py --port 6432

    # Direct to PostgreSQL (should pass):
    python test_selectin_bug.py --port 5432
"""

import argparse
import asyncio
import sys
import traceback
from datetime import datetime, timedelta, timezone
from typing import List, Optional
from uuid import UUID, uuid4

import asyncpg
from sqlalchemy.orm import sessionmaker
from sqlmodel import (
    Column,
    DateTime,
    Field,
    Relationship,
    SQLModel,
    UniqueConstraint,
    create_engine,
    func,
    select,
)
from sqlalchemy.ext.asyncio import AsyncEngine, AsyncSession


# ---------------------------------------------------------------------------
# Custom asyncpg Connection (same as captive-portal)
# ---------------------------------------------------------------------------
class CConnection(asyncpg.Connection):
    """Generate UUID-based unique statement names to avoid collisions
    across pooled connections."""

    def _get_unique_id(self, prefix: str) -> str:
        return f"__asyncpg_{prefix}_{uuid4()}__".replace("-", "_")


# ---------------------------------------------------------------------------
# Models — simplified captive-portal schema with selectin loading
# ---------------------------------------------------------------------------
class Check(SQLModel, table=True):
    __tablename__ = "test_check"
    id: Optional[int] = Field(default=None, primary_key=True)
    username: str = Field(index=True)
    attribute: str = ""
    op: str = ":="
    value: str = ""
    parent_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")
    parent: Optional["Parent"] = Relationship(back_populates="checks")


class Reply(SQLModel, table=True):
    __tablename__ = "test_reply"
    id: Optional[int] = Field(default=None, primary_key=True)
    username: str = Field(index=True)
    attribute: str = ""
    op: str = "="
    value: str = ""
    parent_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")
    parent: Optional["Parent"] = Relationship(back_populates="replies")


class GroupLink(SQLModel, table=True):
    __tablename__ = "test_group_link"
    id: Optional[int] = Field(default=None, primary_key=True)
    username: str = Field(index=True, default="")
    groupname: str = ""
    priority: int = 0
    parent_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")
    parent: Optional["Parent"] = Relationship(back_populates="group_links")


class PhoneCode(SQLModel, table=True):
    __tablename__ = "test_phone_code"
    id: Optional[int] = Field(default=None, primary_key=True)
    code: str = Field(index=True)
    valid_until: Optional[datetime] = Field(
        default=None, sa_column=Column(DateTime(timezone=True))
    )
    used: bool = False
    created_at: Optional[datetime] = Field(
        sa_column=Column(DateTime(timezone=True), server_default=func.now(), nullable=False)
    )
    parent_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")
    parent: Optional["Parent"] = Relationship(back_populates="phone_codes")


class PhoneCall(SQLModel, table=True):
    __tablename__ = "test_phone_call"
    id: Optional[int] = Field(default=None, primary_key=True)
    phone_number: str = ""
    gateway_number: str = ""
    redirect_params: str = ""
    verified: bool = False
    authorized: bool = False
    valid_until: Optional[datetime] = Field(
        default=None, sa_column=Column(DateTime(timezone=True))
    )
    created_at: Optional[datetime] = Field(
        sa_column=Column(DateTime(timezone=True), server_default=func.now(), nullable=False)
    )
    parent_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")
    call_uuid: UUID = Field(default_factory=uuid4, index=True)
    parent: Optional["Parent"] = Relationship(back_populates="phone_calls")


class Parent(SQLModel, table=True):
    """Analogue of RadUserModel from captive-portal with 7 selectin-loaded relationships."""

    __tablename__ = "test_parent"
    id: Optional[int] = Field(default=None, primary_key=True)
    username: str = Field(index=True, unique=True)
    type: str = "mobile"
    client_mac: Optional[str] = None
    last_logged: Optional[datetime] = Field(
        default=None, sa_column=Column(DateTime(timezone=True))
    )
    created_at: Optional[datetime] = Field(
        sa_column=Column(DateTime(timezone=True), server_default=func.now(), nullable=False)
    )
    # Self-referential FK (like RadUserModel.user_id)
    ref_id: Optional[int] = Field(default=None, foreign_key="test_parent.id")

    # --- 7 selectin-loaded relationships (matching captive-portal) ---
    checks: List[Check] = Relationship(
        back_populates="parent",
        sa_relationship_kwargs={"cascade": "all,delete", "lazy": "selectin"},
    )
    replies: List[Reply] = Relationship(
        back_populates="parent",
        sa_relationship_kwargs={"cascade": "all,delete", "lazy": "selectin"},
    )
    group_links: List[GroupLink] = Relationship(
        back_populates="parent",
        sa_relationship_kwargs={"cascade": "all,delete", "lazy": "selectin"},
    )
    phone_codes: List[PhoneCode] = Relationship(
        back_populates="parent",
        sa_relationship_kwargs={"cascade": "all,delete", "lazy": "selectin"},
    )
    phone_calls: List[PhoneCall] = Relationship(
        back_populates="parent",
        sa_relationship_kwargs={
            "cascade": "all,delete",
            "lazy": "selectin",
            "order_by": "PhoneCall.created_at.desc()",
        },
    )
    # Self-referential: children refs (like client_macs)
    child_refs: List["Parent"] = Relationship(
        back_populates="parent_ref",
        sa_relationship_kwargs={"lazy": "selectin", "join_depth": 1},
    )
    # Self-referential: parent ref (like user)
    parent_ref: Optional["Parent"] = Relationship(
        back_populates="child_refs",
        sa_relationship_kwargs={
            "remote_side": "Parent.id",
            "lazy": "selectin",
            "join_depth": 1,
        },
    )


# ---------------------------------------------------------------------------
# Engine factory
# ---------------------------------------------------------------------------
def make_engine(host: str, port: int, user: str, password: str, dbname: str) -> AsyncEngine:
    dsn = f"postgresql+asyncpg://{user}:{password}@{host}:{port}/{dbname}"
    return AsyncEngine(
        create_engine(
            dsn,
            future=True,
            pool_pre_ping=True,
            pool_size=1,
            max_overflow=4,
            connect_args={
                "statement_cache_size": 0,
                "prepared_statement_cache_size": 0,
                "connection_class": CConnection,
            },
        )
    )


# ---------------------------------------------------------------------------
# Setup: create tables and seed data
# ---------------------------------------------------------------------------
async def setup_schema(engine: AsyncEngine) -> None:
    async with engine.begin() as conn:
        await conn.run_sync(SQLModel.metadata.drop_all)
        await conn.run_sync(SQLModel.metadata.create_all)


async def seed_data(engine: AsyncEngine, count: int = 5) -> List[str]:
    """Insert test parents with records in every child table. Returns usernames."""
    usernames = []
    async_session = sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with async_session() as session:
        for i in range(count):
            username = f"testuser_{i}"
            usernames.append(username)

            parent = Parent(username=username, type="mobile")
            session.add(parent)
            await session.flush()

            # Populate all child tables
            session.add(Check(username=username, attribute="Cleartext-Password", op=":=", value="secret", parent_id=parent.id))
            session.add(Check(username=username, attribute="Session-Timeout", op=":=", value="86400", parent_id=parent.id))
            session.add(Reply(username=username, attribute="Reply-Message", op="=", value="Hello", parent_id=parent.id))
            session.add(GroupLink(username=username, groupname="default", priority=0, parent_id=parent.id))
            session.add(PhoneCode(
                code="1234",
                valid_until=datetime.now(timezone.utc) + timedelta(minutes=5),
                parent_id=parent.id,
            ))
            session.add(PhoneCall(
                phone_number=f"+7900000000{i}",
                gateway_number=f"+7800000000{i}",
                redirect_params="{}",
                valid_until=datetime.now(timezone.utc) + timedelta(seconds=60),
                parent_id=parent.id,
            ))

        await session.commit()

    return usernames


# ---------------------------------------------------------------------------
# Core test: selectin loading query (matches captive-portal's get_one_by_username_and_type)
# ---------------------------------------------------------------------------
async def query_with_selectin(engine: AsyncEngine, username: str) -> None:
    """Execute select(Parent).where(username==X).scalars().one()
    which triggers selectin loading for all 7 relationships."""
    async_session = sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with async_session() as session:
        statement = select(Parent).where(Parent.username == username)
        result = (await session.execute(statement)).scalars().one()
        # Access the relationships to ensure they were loaded
        _ = result.checks
        _ = result.replies
        _ = result.group_links
        _ = result.phone_codes
        _ = result.phone_calls
        _ = result.child_refs
        _ = result.parent_ref


# ---------------------------------------------------------------------------
# Worker: run queries in a loop
# ---------------------------------------------------------------------------
async def worker(
    worker_id: int,
    engine: AsyncEngine,
    usernames: List[str],
    iterations: int,
    results: dict,
) -> None:
    for i in range(iterations):
        username = usernames[i % len(usernames)]
        try:
            await query_with_selectin(engine, username)
        except TypeError as e:
            if "expected string or bytes-like object" in str(e):
                results["target_errors"] += 1
                print(
                    f"  [worker {worker_id}, iter {i}] TARGET ERROR (status=None): {e}",
                    flush=True,
                )
                # Print traceback for first occurrence
                if results["target_errors"] <= 3:
                    traceback.print_exc()
            else:
                results["other_errors"] += 1
                print(
                    f"  [worker {worker_id}, iter {i}] TypeError: {e}",
                    flush=True,
                )
        except Exception as e:
            results["other_errors"] += 1
            err_str = str(e)
            # Only print first few errors to avoid flooding
            if results["other_errors"] <= 10:
                print(
                    f"  [worker {worker_id}, iter {i}] {type(e).__name__}: {err_str[:200]}",
                    flush=True,
                )

    results["completed"] += iterations


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
async def run_test(args: argparse.Namespace) -> bool:
    engine = make_engine(args.host, args.port, args.user, args.password, args.dbname)

    print(f"Target: {args.host}:{args.port}/{args.dbname}")
    print(f"Iterations: {args.iterations}, Workers: {args.workers}")
    print()

    # Setup
    print("Setting up schema...", flush=True)
    await setup_schema(engine)

    print("Seeding data...", flush=True)
    usernames = await seed_data(engine, count=5)

    # Warm-up: run a few queries to populate pg_doorman's prepared statement cache
    print("Warming up prepared statement cache...", flush=True)
    for username in usernames:
        await query_with_selectin(engine, username)

    # Run test
    results = {"target_errors": 0, "other_errors": 0, "completed": 0}
    iters_per_worker = args.iterations // args.workers

    print(f"Running {args.workers} workers x {iters_per_worker} iterations...", flush=True)
    print()

    tasks = []
    for w in range(args.workers):
        tasks.append(worker(w, engine, usernames, iters_per_worker, results))

    await asyncio.gather(*tasks)

    # Results
    print()
    print("=" * 60)
    total = results["completed"]
    target = results["target_errors"]
    other = results["other_errors"]
    ok = total - target - other

    print(f"Total queries:     {total}")
    print(f"Successful:        {ok}")
    print(f"Target errors:     {target}  (status=None TypeError)")
    print(f"Other errors:      {other}")
    print("=" * 60)

    if target > 0:
        print(f"BUG REPRODUCED: {target} occurrences of status=None TypeError")
        return False
    elif other > 0:
        print(f"Some errors occurred but not the target bug ({other} other errors)")
        return False
    else:
        print("All queries passed successfully")
        return True


def main():
    parser = argparse.ArgumentParser(
        description="Reproduce selectin loading TypeError with pg_doorman"
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=6432)
    parser.add_argument("--user", default="testuser")
    parser.add_argument("--password", default="password")
    parser.add_argument("--dbname", default="testdb")
    parser.add_argument("--iterations", type=int, default=500)
    parser.add_argument("--workers", type=int, default=4)
    args = parser.parse_args()

    success = asyncio.run(run_test(args))
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()