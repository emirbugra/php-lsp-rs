<?php
namespace App;

class Repo {
    /** @var \PDO */
    private $pdo;
    public int $count = 0;
    public const LIMIT = 10;
    public function __construct(\PDO $pdo) { $this->pdo = $pdo; }
    /** Finds rows.
     * @param string $sql the query
     * @return \PDOStatement|false */
    public function find(string $sql, array $args = []) { return $this->pdo->prepare($sql); }
    public static function make(): static { return new static(new \PDO("x")); }
}
