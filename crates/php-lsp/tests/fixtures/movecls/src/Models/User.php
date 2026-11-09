<?php

namespace App\Models;

class User
{
    public function helper(): Helper
    {
        return new Helper();
    }
}
