<?php

namespace App\Http;

use App\Models\User;

class Controller
{
    /** @var User */
    private $user;

    public function make(): User
    {
        $class = 'App\Models\User';
        $other = "App\\Models\\User";
        return new User();
    }

    public function full(): \App\Models\User
    {
        return new \App\Models\User();
    }
}
