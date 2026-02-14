# EGUI Migration Rollback Guide

This document records the checkpoint created before the `egui` migration and how to roll back quickly.

## Checkpoint Created

- Commit: `f07fabb`
- Backup branch: `backup/pre-egui-20260214-233917`
- Tag: `checkpoint/pre-egui-20260214-233917`
- Offline bundle: `pre-egui-20260214-233917.bundle`

## Quick Rollback (Recommended: Tag)

```powershell
git switch -c rollback/pre-egui checkpoint/pre-egui-20260214-233917
```

If you want to reset `main` to this state:

```powershell
git switch main
git reset --hard checkpoint/pre-egui-20260214-233917
```

## Rollback via Backup Branch

```powershell
git switch backup/pre-egui-20260214-233917
```

Or create a fresh rollback branch from it:

```powershell
git switch -c rollback/pre-egui backup/pre-egui-20260214-233917
```

## Restore from Offline Bundle (No Remote Needed)

Clone from bundle:

```powershell
git clone pre-egui-20260214-233917.bundle ai-shepherd-rollback
cd ai-shepherd-rollback
git switch -c rollback/pre-egui checkpoint/pre-egui-20260214-233917
```

Or fetch bundle into current repo:

```powershell
git fetch .\pre-egui-20260214-233917.bundle "refs/*:refs/*"
git switch -c rollback/pre-egui checkpoint/pre-egui-20260214-233917
```

## Verify You Are on the Checkpoint

```powershell
git rev-parse --short HEAD
```

Expected output:

```text
f07fabb
```
