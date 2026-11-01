<?php
namespace App;

class CRepo {
    public function __construct(public string $dsn = '') {}
    public function find(int $id): ?CItem { return $this->load($id); }
    private function load(int $id): ?CItem { return new CItem(); }
}

class CItem { public function title(): string { return strtoupper('x'); } }

class CService {
    private CRepo $repo;
    public function show(int $id) {
        $repo = new CRepo(); $this->repo->find(1);
        $item = $repo->find($id);
        $list = [new CItem()];
        return $item?->title() . chelper($repo);
    }
    public function again() { return (new CRepo())->find(2); }
}

function chelper(CRepo $r) { return $r->find(1); }

$top = chelper(new CRepo('x'));
