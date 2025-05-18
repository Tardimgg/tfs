import {Component, Inject} from '@angular/core';
import {MAT_DIALOG_DATA, MatDialog, MatDialogModule, MatDialogRef} from '@angular/material/dialog';
import {MatSelectModule} from '@angular/material/select';
import {anonymous} from '../../entities/user-permission';
import {PermissionType} from '../../entities/permission-type';
import {CommonModule, NgFor} from '@angular/common';
import {MatButtonModule} from '@angular/material/button';
import {PermissionControllerComponent} from '../permission-controller/permission-controller.component';
import {ObjType} from '../../entities/obj-type';
import {FsService} from '../../services/fs-service';
import {timer} from 'rxjs';
import {FsNodeType} from '../fs/fs.component';
import {castToE, PermissionService} from '../../services/permission-service';
import {MatInputModule} from '@angular/material/input';
import {FormsModule} from '@angular/forms';

@Component({
  selector: 'app-permission-management',
  imports: [MatDialogModule, MatSelectModule, NgFor, MatButtonModule, CommonModule, MatInputModule, FormsModule],
  templateUrl: './permission-management.component.html',
  styleUrl: './permission-management.component.css'
})
export class PermissionManagementComponent {

  path: string
  objId: string = "";
  objType: ObjType = ObjType.File;
  isPublic: boolean = false;
  isLoaded = false;

  initialPermissions: Map<string, PermissionType[]> = new Map<string, PermissionType[]>();

  displayedPermissions: Map<string, PermissionType[]> = new Map<string, PermissionType[]>();
  displayedNewPermissions: [string, PermissionType[]][] = [];
  userPermissionsToDelete: string[] = []

  constructor(
    public dialogRef: MatDialogRef<PermissionManagementComponent>,
    @Inject(MAT_DIALOG_DATA) public data: object,
    private matDialog: MatDialog,
    private fsService: FsService,
    private permissionService: PermissionService) {

    this.path = "path" in data ? data["path"] as string : "";
    // this.objId = "objId" in data ? data["objId"] as number : 0;
    // this.objType = "objType" in data ? data["objType"] as ObjType : ObjType.File;

    this.dialogRef.updateSize('80%', '70%');
    // this.isPublic = "isPublic" in data ? data["isPublic"] as boolean : false;
    // this.displayedPermissions = "permissions" in data ? data["permissions"] as UserPermission[] : [];
    // this.permissions = [
    //   new UserPermission("lox", PermissionType.Read),
    //   new UserPermission("lox2", PermissionType.Write),
    //   new UserPermission("lox3", PermissionType.ReadRecursively),
    //   new UserPermission("lox4", PermissionType.GrantWriteRecursively)
    // ]

    timer(0)
      .subscribe(async(_) => {
        let node = await this.fsService.getNodeMeta(this.path);
        switch (node.file_type) {
          case FsNodeType.File:
            this.objType = ObjType.File;
            break;
          case FsNodeType.Folder:
            this.objType = ObjType.Folder;
            break;
        }
        this.objId = node.id;
        console.log(node)
        console.log(this.objType)

        let permissions = await this.permissionService.getPermissions(this.objType, node.id);
        let permissionsMap = new Map<string, PermissionType[]>;

        for (let permission of permissions) {
          let prev = permissionsMap.get(permission.login);
          if (prev != undefined) {
            prev.push(permission.permission_type);
          } else {
            permissionsMap.set(permission.login, [permission.permission_type]);
          }
        }

        if (permissionsMap.has(anonymous)) {
          let rules = permissionsMap.get(anonymous) as PermissionType[];
          for (let rule of rules) {
            if (rule == PermissionType.ReadRecursively || rule == PermissionType.Read) {
              this.isPublic = true;
              break;
            }
          }
        }

        for (let k of permissionsMap.keys()) {
          let v = permissionsMap.get(k);
          if (v != undefined) {
            this.initialPermissions.set(k, [...v]);
          }
        }

        this.displayedPermissions = permissionsMap;
        this.isLoaded = true;
    })
  }

  castPermissionsToString(permissions: PermissionType[]) {
    return permissions.map(v => castToE(v)).join(", ")
  }

  openPermissionController(userLogin: string) {
    if (!this.isLoaded) {
      console.log("data is not ready yet")
      return;
    }
    console.log("openPermissionManagement" + this.path)

    let userPermissions: PermissionType[] = [];
    if (this.displayedPermissions.has(userLogin)) {
      userPermissions = userPermissions.concat(this.displayedPermissions.get(userLogin) as PermissionType[]);
    }
    for (let permission of this.displayedNewPermissions) {
      if (permission[0] == userLogin) {
        userPermissions = userPermissions.concat(permission[1]);
      }
    }

    let controllerDialog = this.matDialog.open(PermissionControllerComponent, {
      data: {
        filename: this.path,
        objId: this.objId,
        objType: this.objType,
        userLogin: userLogin,
        userPermissions: userPermissions.map((v) => v)
      }
    })
    controllerDialog.afterClosed().subscribe(change => {
      let toAdd = change.toAdd;
      let toDelete = change.toDelete;



      let prev = this.displayedPermissions.get(userLogin);
      if (prev != undefined) {

        this.displayedPermissions.set(
          userLogin,
          prev.filter((v) => toDelete.indexOf(v) == -1)
            .concat(toAdd.filter((v: PermissionType) => (prev as PermissionType[]).indexOf(v) == -1))
        );
      } else {
        for (let newUser of this.displayedNewPermissions) {
          if (newUser[0] == userLogin) {
            newUser[1] = newUser[1].filter((v) => toDelete.indexOf(v) == -1)
              .concat(toAdd.filter((v: PermissionType) => (newUser[1] as PermissionType[]).indexOf(v) == -1))
            break;
          }
        }
        // this.displayedPermissions.set(userLogin, toAdd);
      }

    })
  }

  markUserPermissionsAsRevocable(userLogin: string): void {
    this.displayedPermissions.delete(userLogin);
    this.userPermissionsToDelete.push(userLogin);
  }

  deleteNewPermission(index: number): void {
    this.displayedNewPermissions.splice(index, 1);
  }


  clickAddPermission(): void {
    this.displayedNewPermissions.push(["", [PermissionType.Read]]);
  }

  updateInput(index: number, event: any) {
    this.displayedNewPermissions[index][0] = event.target.value;
    // console.log(this.displayedNewPermissions)
}

  save(): void {

    for (let permissions of this.displayedNewPermissions) {
      let userLogin = permissions[0];
      for (let permission of permissions[1]) {
        this.permissionService.putPermission(userLogin, this.objType, this.objId, permission);
      }
    }

    for (let permissions of this.displayedPermissions.entries()) {
      let userLogin = permissions[0];
      let init = this.initialPermissions.get(permissions[0]);
      if (init != undefined) {
        let init_permission_set = new Set(init);
        let current_permission_set = new Set(permissions[1]);
        for (let permission of permissions[1]) {
          if (!init_permission_set.has(permission)) {
            this.permissionService.putPermission(userLogin, this.objType, this.objId, permission);
          }
        }

        for (let permission of init) {
          if (!current_permission_set.has(permission)) {
            this.permissionService.deletePermission(userLogin, this.objType, this.objId, permission);
          }
        }
      }
    }

    let flag = false;
    for (let user of this.userPermissionsToDelete) {
      for (let newPermissions of this.displayedNewPermissions) {
        if (user == newPermissions[0]) {
          let init = this.initialPermissions.get(user);
          if (init != undefined) {
            let newPermissionsSet = new Set(newPermissions[1]);

            for (let initPermission of init) {
              if (!newPermissionsSet.has(initPermission)) {
                this.permissionService.deletePermission(user, this.objType, this.objId, initPermission);
              }
            }
          }


          flag = true;
          break;
        }
      }
      if (!flag) {
        let init = this.initialPermissions.get(user);
        if (init != undefined) {
          for (let initPermission of init) {
            this.permissionService.deletePermission(user, this.objType, this.objId, initPermission);
          }
        }
      }
    }

    this.dialogRef.close();
  }

  protected readonly castToE = castToE;
}
